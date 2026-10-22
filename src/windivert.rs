//! WinDivert transparent gateway.
//!
//! Windows applications do not honour the explicit HTTP proxy automatically.
//! This module redirects outbound TCP connections to the local MITM listener at
//! the packet layer and performs a small userspace NAT so return packets appear
//! to come from the original destination.
//!
//! This is the local-machine equivalent of mitmproxy's WireGuard mode: no client
//! configuration, any application, any supported port. (A real WireGuard/TUN
//! gateway would additionally accept peers from other devices; that needs a
//! userspace TCP/IP stack and is out of scope here.)
//!
//! IPv4 and IPv6 are both translated. The relay binds `127.0.0.1`/`[::1]` on the
//! proxy port; the proxy's own upstream sockets are bound to a reserved
//! source-port range (30000-39999) which the filter excludes, preventing a
//! redirect loop. The loopback return path is matched by the proxy listener's
//! source port.
//!
//! QUIC (UDP/443) cannot be decrypted by a TLS MITM. When `--block-quic` is on,
//! those datagrams are dropped and counted so the application falls back to TCP.

use crate::state::AppState;
use crate::transparent::TransparentMap;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windivert::prelude::*;

const RELAY_PORT_MIN: u16 = 20000;
const RELAY_PORT_MAX: u16 = 29999;
const UPSTREAM_PORT_MIN: u16 = 30000;
const UPSTREAM_PORT_MAX: u16 = 39999;
const NAT_TTL: Duration = Duration::from_secs(300);
/// How long `stop` waits for the worker to drain the driver queue and close the
/// WinDivert handle. The queue is small and the drain only re-injects packets,
/// so this is a safety net, not an expected wait.
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FlowKey {
    src_ip: IpAddr,
    src_port: u16,
    dst_ip: IpAddr,
    dst_port: u16,
}

#[derive(Debug, Clone, Copy)]
struct NatEntry {
    key: FlowKey,
    relay_port: u16,
    if_idx: u32,
    sub_if_idx: u32,
    last_seen: Instant,
}

struct NatState {
    flows: HashMap<FlowKey, NatEntry>,
    relays: HashMap<u16, FlowKey>,
    next_relay: u16,
    last_cleanup: Instant,
    proxy_port: u16,
}

impl NatState {
    fn new(proxy_port: u16) -> Self {
        Self {
            flows: HashMap::new(),
            relays: HashMap::new(),
            next_relay: RELAY_PORT_MIN,
            last_cleanup: Instant::now(),
            proxy_port,
        }
    }

    fn alloc_relay(&mut self) -> u16 {
        for _ in 0..(RELAY_PORT_MAX - RELAY_PORT_MIN + 1) {
            let p = self.next_relay;
            self.next_relay = if p >= RELAY_PORT_MAX {
                RELAY_PORT_MIN
            } else {
                p + 1
            };
            if p != self.proxy_port && !self.relays.contains_key(&p) {
                return p;
            }
        }
        RELAY_PORT_MIN
    }

    fn cleanup(&mut self) {
        if self.last_cleanup.elapsed() < Duration::from_secs(30) {
            return;
        }
        self.last_cleanup = Instant::now();
        self.flows.retain(|_, e| e.last_seen.elapsed() < NAT_TTL);
        let live: std::collections::HashSet<u16> =
            self.flows.values().map(|e| e.relay_port).collect();
        self.relays.retain(|p, _| live.contains(p));
    }
}

#[derive(Debug, Clone, Copy)]
struct PacketView {
    version: u8,
    tcp_off: usize,
    src_ip: IpAddr,
    dst_ip: IpAddr,
    src_port: u16,
    dst_port: u16,
    flags: u8,
}

impl PacketView {
    fn syn(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

fn parse_packet(data: &[u8]) -> Option<PacketView> {
    if data.is_empty() {
        return None;
    }
    match data[0] >> 4 {
        4 => parse_ipv4_tcp(data),
        6 => parse_ipv6_tcp(data),
        _ => None,
    }
}

fn parse_ipv4_tcp(data: &[u8]) -> Option<PacketView> {
    if data.len() < 20 {
        return None;
    }
    let ihl = ((data[0] & 0x0f) as usize) * 4;
    if ihl < 20 || data.len() < ihl {
        return None;
    }
    let total_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if total_len < ihl || total_len > data.len() {
        return None;
    }
    if data[9] != 6 {
        return None;
    }
    let src_ip = IpAddr::V4(Ipv4Addr::new(data[12], data[13], data[14], data[15]));
    let dst_ip = IpAddr::V4(Ipv4Addr::new(data[16], data[17], data[18], data[19]));
    parse_tcp(data, 4, ihl, total_len, src_ip, dst_ip)
}

fn parse_ipv6_tcp(data: &[u8]) -> Option<PacketView> {
    if data.len() < 40 {
        return None;
    }
    let payload_len = u16::from_be_bytes([data[4], data[5]]) as usize;
    let total_len = 40 + payload_len;
    if total_len > data.len() {
        return None;
    }
    // Only a plain TCP next-header is handled; extension headers are rare for
    // HTTPS traffic and would need a header chain walk.
    if data[6] != 6 {
        return None;
    }
    let mut src = [0u8; 16];
    let mut dst = [0u8; 16];
    src.copy_from_slice(&data[8..24]);
    dst.copy_from_slice(&data[24..40]);
    parse_tcp(
        data,
        6,
        40,
        total_len,
        IpAddr::V6(Ipv6Addr::from(src)),
        IpAddr::V6(Ipv6Addr::from(dst)),
    )
}

fn parse_tcp(
    data: &[u8],
    version: u8,
    tcp_off: usize,
    total_len: usize,
    src_ip: IpAddr,
    dst_ip: IpAddr,
) -> Option<PacketView> {
    if total_len < tcp_off + 20 {
        return None;
    }
    let tcp_hlen = ((data[tcp_off + 12] >> 4) as usize) * 4;
    if tcp_hlen < 20 || tcp_off + tcp_hlen > total_len {
        return None;
    }
    Some(PacketView {
        version,
        tcp_off,
        src_ip,
        dst_ip,
        src_port: u16::from_be_bytes([data[tcp_off], data[tcp_off + 1]]),
        dst_port: u16::from_be_bytes([data[tcp_off + 2], data[tcp_off + 3]]),
        flags: data[tcp_off + 13],
    })
}

/// Destination port of an outbound UDP datagram (IPv4 or IPv6), if the packet is
/// actually UDP.
fn parse_udp_dst(data: &[u8]) -> Option<u16> {
    match data.first().map(|b| b >> 4) {
        Some(4) => {
            if data.len() < 20 {
                return None;
            }
            let ihl = ((data[0] & 0x0f) as usize) * 4;
            if ihl < 20 || data.len() < ihl + 8 || data[9] != 17 {
                return None;
            }
            Some(u16::from_be_bytes([data[ihl + 2], data[ihl + 3]]))
        }
        Some(6) => {
            if data.len() < 48 || data[6] != 17 {
                return None;
            }
            Some(u16::from_be_bytes([data[42], data[43]]))
        }
        _ => None,
    }
}

fn loopback_ip(version: u8) -> IpAddr {
    if version == 6 {
        IpAddr::V6(Ipv6Addr::LOCALHOST)
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    }
}

fn set_src(data: &mut [u8], version: u8, ip: IpAddr) {
    match (version, ip) {
        (4, IpAddr::V4(v4)) => data[12..16].copy_from_slice(&v4.octets()),
        (6, IpAddr::V6(v6)) => data[8..24].copy_from_slice(&v6.octets()),
        _ => {}
    }
}

fn set_dst(data: &mut [u8], version: u8, ip: IpAddr) {
    match (version, ip) {
        (4, IpAddr::V4(v4)) => data[16..20].copy_from_slice(&v4.octets()),
        (6, IpAddr::V6(v6)) => data[24..40].copy_from_slice(&v6.octets()),
        _ => {}
    }
}

fn set_src_port(data: &mut [u8], p: PacketView, v: u16) {
    data[p.tcp_off..p.tcp_off + 2].copy_from_slice(&v.to_be_bytes());
}
fn set_dst_port(data: &mut [u8], p: PacketView, v: u16) {
    data[p.tcp_off + 2..p.tcp_off + 4].copy_from_slice(&v.to_be_bytes());
}

fn rewrite_to_proxy(
    packet: &mut WinDivertPacket<'_, NetworkLayer>,
    p: PacketView,
    relay_port: u16,
    proxy_port: u16,
    loop_if: u32,
) {
    let loop_ip = loopback_ip(p.version);
    {
        let data = packet.data.to_mut();
        set_src(data, p.version, loop_ip);
        set_dst(data, p.version, loop_ip);
        set_src_port(data, p, relay_port);
        set_dst_port(data, p, proxy_port);
    }
    packet.address.set_outbound(true);
    packet.address.set_loopback(true);
    packet.address.as_mut().set_ipv6(p.version == 6);
    packet.address.set_interface_index(loop_if);
    packet.address.set_subinterface_index(0);
    let _ = packet.recalculate_checksums(Default::default());
}

fn rewrite_to_client(
    packet: &mut WinDivertPacket<'_, NetworkLayer>,
    p: PacketView,
    entry: NatEntry,
) {
    {
        let data = packet.data.to_mut();
        set_src(data, p.version, entry.key.dst_ip);
        set_dst(data, p.version, entry.key.src_ip);
        set_src_port(data, p, entry.key.dst_port);
        set_dst_port(data, p, entry.key.src_port);
    }
    packet.address.set_outbound(false);
    packet.address.set_loopback(false);
    packet.address.as_mut().set_ipv6(p.version == 6);
    packet.address.set_interface_index(entry.if_idx);
    packet.address.set_subinterface_index(entry.sub_if_idx);
    let _ = packet.recalculate_checksums(Default::default());
}

/// Returns `false` when the packet must be dropped instead of re-injected.
fn process_packet(
    packet: &mut WinDivertPacket<'_, NetworkLayer>,
    nat: &Arc<Mutex<NatState>>,
    transparent: &TransparentMap,
    loop_if: u32,
    ports: &[u16],
    block_quic: bool,
    quic_dropped: &AtomicU64,
) -> bool {
    let data = &packet.data;
    if let Some(dst_port) = parse_udp_dst(data) {
        if block_quic && dst_port == 443 && !packet.address.loopback() {
            quic_dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        return true;
    }
    let Some(p) = parse_packet(data) else {
        return true;
    };
    if !packet.address.outbound() {
        return true;
    }
    if packet.address.loopback() {
        if p.src_port != nat.lock().map(|n| n.proxy_port).unwrap_or(0) {
            return true;
        }
        let entry = {
            let mut guard = match nat.lock() {
                Ok(g) => g,
                Err(_) => return true,
            };
            guard.cleanup();
            let key = guard.relays.get(&p.dst_port).copied();
            if let Some(key) = key {
                guard.flows.get_mut(&key).map(|e| {
                    e.last_seen = Instant::now();
                    *e
                })
            } else {
                None
            }
        };
        if let Some(entry) = entry {
            rewrite_to_client(packet, p, entry);
        }
        return true;
    }
    // Only intercept the configured ports and never our own upstream sockets.
    if !ports.contains(&p.dst_port) {
        return true;
    }
    if (UPSTREAM_PORT_MIN..=UPSTREAM_PORT_MAX).contains(&p.src_port) {
        return true;
    }
    let key = FlowKey {
        src_ip: p.src_ip,
        src_port: p.src_port,
        dst_ip: p.dst_ip,
        dst_port: p.dst_port,
    };
    let (entry, new_flow) = {
        let mut guard = match nat.lock() {
            Ok(g) => g,
            Err(_) => return true,
        };
        guard.cleanup();
        if let Some(entry) = guard.flows.get(&key).copied() {
            if let Some(e) = guard.flows.get_mut(&key) {
                e.last_seen = Instant::now();
            }
            (entry, false)
        } else if p.syn() {
            let relay = guard.alloc_relay();
            let entry = NatEntry {
                key,
                relay_port: relay,
                if_idx: packet.address.interface_index(),
                sub_if_idx: packet.address.subinterface_index(),
                last_seen: Instant::now(),
            };
            guard.flows.insert(key, entry);
            guard.relays.insert(relay, key);
            (entry, true)
        } else {
            return true;
        }
    };
    if new_flow || p.syn() {
        // A SYN retransmission may arrive after the listener consumed the map
        // entry; re-arm it so the retry can still be accepted.
        transparent.insert(
            entry.relay_port,
            crate::transparent::TransparentTarget {
                client: SocketAddr::new(entry.key.src_ip, entry.key.src_port),
                original_dst: SocketAddr::new(entry.key.dst_ip, entry.key.dst_port),
            },
        );
    }
    if new_flow {
        tracing::debug!(
            "windivert redirect {}:{} -> {}:{} via relay {}",
            entry.key.src_ip,
            entry.key.src_port,
            entry.key.dst_ip,
            entry.key.dst_port,
            entry.relay_port
        );
    }
    let proxy_port = nat.lock().map(|n| n.proxy_port).unwrap_or(0);
    rewrite_to_proxy(packet, p, entry.relay_port, proxy_port, loop_if);
    true
}

#[cfg(windows)]
fn loopback_interface_index() -> u32 {
    use windows_sys::Win32::NetworkManagement::IpHelper::GetBestInterface;
    let dest = u32::from_ne_bytes([127, 0, 0, 1]);
    let mut idx = 0u32;
    let rc = unsafe { GetBestInterface(dest, &mut idx) };
    if rc == 0 && idx != 0 {
        idx
    } else {
        1
    }
}

fn build_filter(proxy_port: u16, ports: &[u16], block_quic: bool) -> String {
    // WinDivert's `!` only applies to a single test, not a parenthesized
    // expression (`!(A and B)` is rejected as InvalidParameter). Use the
    // De Morgan equivalent and explicit comparisons.
    let mut tcp_ports: Vec<String> = ports
        .iter()
        .map(|port| format!("tcp.DstPort == {port}"))
        .collect();
    if tcp_ports.is_empty() {
        tcp_ports.push("tcp.DstPort == 443".into());
    }
    let tcp_ports = tcp_ports.join(" or ");
    let quic = if block_quic {
        " or (not loopback and udp and udp.DstPort == 443)"
    } else {
        ""
    };
    format!(
        "outbound and ((not loopback and tcp and ({tcp_ports}) and (tcp.SrcPort < {UPSTREAM_PORT_MIN} or tcp.SrcPort > {UPSTREAM_PORT_MAX}){quic}) or (loopback and tcp and tcp.SrcPort == {proxy_port}))"
    )
}

#[cfg(windows)]
pub async fn start(state: Arc<AppState>) -> anyhow::Result<()> {
    {
        let rt = state.windivert.read().await;
        if rt.enabled {
            return Ok(());
        }
    }
    {
        let proxy = state.proxy.lock().await;
        if proxy.state != "running" && proxy.state != "paused" {
            anyhow::bail!("start the MitmRs proxy before enabling WinDivert transparent redirect");
        }
    }
    let cfg = state.config.read().await.clone();
    let proxy_port = cfg.proxy_port;
    let ports = state.windivert_ports.read().await.clone();
    let block_quic = state.block_quic.load(Ordering::Relaxed);
    tracing::info!("windivert gateway: tcp ports {ports:?}, ipv4+ipv6, block_quic={block_quic}");
    {
        // Reflect the requested configuration even if the driver fails to open.
        let mut rt = state.windivert.write().await;
        rt.ports = ports.clone();
        rt.block_quic = block_quic;
    }
    let packets = state.windivert.read().await.packets.clone();
    let quic_dropped = state.windivert.read().await.quic_dropped.clone();
    let filter = build_filter(proxy_port, &ports, block_quic);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let state_for_thread = state.clone();
    let loop_if = loopback_interface_index();
    let nat = Arc::new(Mutex::new(NatState::new(proxy_port)));
    let filter_for_log = filter.clone();
    let shutdown_slot: Arc<Mutex<Option<ShutdownHandle>>> = Arc::new(Mutex::new(None));
    let shutdown_for_thread = shutdown_slot.clone();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop_requested.clone();
    let done = Arc::new(tokio::sync::Notify::new());
    let done_for_thread = done.clone();
    let ports_for_thread = ports.clone();
    let thread = std::thread::Builder::new()
        .name("mitmrs-windivert".into())
        .spawn(move || {
            let divert = match WinDivert::network(filter_for_log, 0, WinDivertFlags::new()) {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(Err(format!("{e}")));
                    done_for_thread.notify_one();
                    return;
                }
            };
            if let Ok(mut slot) = shutdown_for_thread.lock() {
                *slot = Some(divert.shutdown_handle());
            }
            let _ = tx.send(Ok(()));
            let mut buf = vec![0u8; 65535];
            let mut last_log = Instant::now();
            loop {
                match divert.recv(&mut buf) {
                    Ok(mut packet) => {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            // Shutdown in progress: the driver may still hand us
                            // packets queued before WinDivertShutdown(). Put them
                            // back untouched so stopping does not black-hole
                            // traffic that was already intercepted.
                            if let Err(e) = divert.send(&packet) {
                                tracing::debug!("windivert drain send failed: {e}");
                            }
                            continue;
                        }
                        packets.fetch_add(1, Ordering::Relaxed);
                        let keep = process_packet(
                            &mut packet,
                            &nat,
                            state_for_thread.transparent.as_ref(),
                            loop_if,
                            &ports_for_thread,
                            block_quic,
                            &quic_dropped,
                        );
                        if keep {
                            if let Err(e) = divert.send(&packet) {
                                tracing::debug!("windivert send failed: {e}");
                            }
                        }
                        if last_log.elapsed() > Duration::from_secs(30) {
                            last_log = Instant::now();
                            if let Ok(guard) = nat.lock() {
                                tracing::debug!(
                                    "windivert active NAT flows: {}",
                                    guard.flows.len()
                                );
                            }
                        }
                    }
                    Err(WinDivertError::Recv(WinDivertRecvError::NoData)) => break,
                    Err(WinDivertError::Recv(WinDivertRecvError::InsufficientBuffer)) => continue,
                    Err(e) => {
                        tracing::warn!("windivert recv error: {e}");
                        break;
                    }
                }
            }
            // Closing the handle detaches the WFP filter. Dropping the last
            // packet buffer before this point keeps the close path free of
            // pending injected data.
            drop(divert);
            tracing::info!("windivert packet loop stopped");
            done_for_thread.notify_one();
        })?;

    match rx.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = thread.join();
            let mut rt = state.windivert.write().await;
            rt.state = "error".into();
            rt.error = Some(e.clone());
            drop(rt);
            anyhow::bail!("WinDivert open failed: {e}");
        }
        Err(_) => {
            let _ = thread.join();
            anyhow::bail!("WinDivert worker exited before opening");
        }
    }

    let shutdown = shutdown_slot.lock().ok().and_then(|mut s| s.take());
    let mut rt = state.windivert.write().await;
    rt.enabled = true;
    rt.state = "running".into();
    rt.error = None;
    rt.ports = ports;
    rt.block_quic = block_quic;
    if let Ok(mut slot) = rt.shutdown.lock() {
        *slot = shutdown;
    }
    rt.stop_requested = stop_requested;
    rt.done = done;
    rt.thread = Some(thread);
    drop(rt);
    state.emit(
        "status",
        serde_json::to_value(state.status().await).unwrap_or_default(),
    );
    Ok(())
}

#[cfg(windows)]
pub async fn stop(state: &Arc<AppState>) -> anyhow::Result<()> {
    // The WinDivert handle lives on the worker thread. `WinDivertShutdown` is
    // the documented way to stop a blocked `WinDivertRecv`, so use it instead
    // of relying on the process teardown, and then wait for the worker to close
    // the handle. The wait is bounded: shutdown must never hang the process.
    let (shutdown, stop_requested, done) = {
        let mut rt = state.windivert.write().await;
        if !rt.enabled {
            return Ok(());
        }
        rt.enabled = false;
        rt.state = "stopping".into();
        // Detach the join handle: the worker reports completion through `done`,
        // and joining it blindly is exactly what could block forever.
        rt.thread.take();
        let shutdown = rt.shutdown.lock().ok().and_then(|mut s| s.take());
        (shutdown, rt.stop_requested.clone(), rt.done.clone())
    };

    // Re-inject instead of rewriting anything that is already queued.
    stop_requested.store(true, Ordering::SeqCst);
    if let Some(shutdown) = shutdown {
        // SHUTDOWN_RECV stops the driver from queueing new packets while still
        // allowing `send` to drain the packets that are already queued.
        if let Err(e) = shutdown.shutdown_recv() {
            tracing::warn!("WinDivert shutdown request failed: {e}");
        }
    }
    if tokio::time::timeout(STOP_TIMEOUT, done.notified())
        .await
        .is_err()
    {
        tracing::warn!(
            "WinDivert worker did not stop within {}s; continuing shutdown",
            STOP_TIMEOUT.as_secs()
        );
    }

    let mut rt = state.windivert.write().await;
    rt.enabled = false;
    rt.state = "off".into();
    rt.error = None;
    drop(rt);
    state.emit(
        "status",
        serde_json::to_value(state.status().await).unwrap_or_default(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syn_packet() -> Vec<u8> {
        let mut p = vec![0u8; 40];
        p[0] = 0x45;
        p[2..4].copy_from_slice(&40u16.to_be_bytes());
        p[8] = 64;
        p[9] = 6;
        p[12..16].copy_from_slice(&[10, 0, 0, 2]);
        p[16..20].copy_from_slice(&[93, 184, 216, 34]);
        let t = 20;
        p[t..t + 2].copy_from_slice(&50000u16.to_be_bytes());
        p[t + 2..t + 4].copy_from_slice(&80u16.to_be_bytes());
        p[t + 12] = 5 << 4;
        p[t + 13] = 0x02; // SYN
        p
    }

    fn syn_packet_v6() -> Vec<u8> {
        let mut p = vec![0u8; 60];
        p[0] = 0x60;
        p[4..6].copy_from_slice(&20u16.to_be_bytes());
        p[6] = 6; // TCP
        p[7] = 64;
        p[8..24].copy_from_slice(&Ipv6Addr::new(0x2400, 0, 0, 0, 0, 0, 0, 2).octets());
        p[24..40].copy_from_slice(&Ipv6Addr::new(0x2606, 0x2800, 0x220, 1, 0, 0, 0, 1).octets());
        let t = 40;
        p[t..t + 2].copy_from_slice(&50001u16.to_be_bytes());
        p[t + 2..t + 4].copy_from_slice(&443u16.to_be_bytes());
        p[t + 12] = 5 << 4;
        p[t + 13] = 0x02;
        p
    }

    fn udp_packet() -> Vec<u8> {
        let mut p = vec![0u8; 28];
        p[0] = 0x45;
        p[2..4].copy_from_slice(&28u16.to_be_bytes());
        p[8] = 64;
        p[9] = 17; // UDP
        p[12..16].copy_from_slice(&[10, 0, 0, 2]);
        p[16..20].copy_from_slice(&[93, 184, 216, 34]);
        let u = 20;
        p[u..u + 2].copy_from_slice(&51000u16.to_be_bytes());
        p[u + 2..u + 4].copy_from_slice(&443u16.to_be_bytes());
        p[u + 4..u + 6].copy_from_slice(&8u16.to_be_bytes());
        p
    }

    fn run(
        packet: &mut WinDivertPacket<'_, NetworkLayer>,
        nat: &Arc<Mutex<NatState>>,
        transparent: &TransparentMap,
        ports: &[u16],
        block_quic: bool,
        dropped: &AtomicU64,
    ) -> bool {
        packet.address.set_outbound(true);
        packet.address.set_interface_index(7);
        process_packet(packet, nat, transparent, 1, ports, block_quic, dropped)
    }

    #[test]
    fn redirects_syn_to_local_proxy_and_nats_return_path() {
        let nat = Arc::new(Mutex::new(NatState::new(18081)));
        let transparent = TransparentMap::default();
        let mut packet = unsafe { WinDivertPacket::<NetworkLayer>::new(syn_packet()) };
        assert!(run(
            &mut packet,
            &nat,
            &transparent,
            &[80, 443],
            false,
            &AtomicU64::new(0)
        ));

        assert!(packet.address.loopback());
        assert!(!packet.address.ipv6());
        assert_eq!(packet.address.interface_index(), 1);
        let view = parse_packet(&packet.data).unwrap();
        assert_eq!(view.src_ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(view.dst_ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(view.dst_port, 18081);
        assert!((RELAY_PORT_MIN..=RELAY_PORT_MAX).contains(&view.src_port));

        let target = transparent.get(view.src_port).unwrap();
        assert_eq!(target.original_dst.port(), 80);
        assert_eq!(target.client.ip().to_string(), "10.0.0.2");
        assert_eq!(target.client.port(), 50000);
    }

    #[test]
    fn redirects_ipv6_syn_and_rewrites_loopback() {
        let nat = Arc::new(Mutex::new(NatState::new(18081)));
        let transparent = TransparentMap::default();
        let mut packet = unsafe { WinDivertPacket::<NetworkLayer>::new(syn_packet_v6()) };
        assert!(run(
            &mut packet,
            &nat,
            &transparent,
            &[443],
            false,
            &AtomicU64::new(0)
        ));

        assert!(packet.address.loopback());
        assert!(packet.address.ipv6());
        let view = parse_packet(&packet.data).unwrap();
        assert_eq!(view.version, 6);
        assert_eq!(view.src_ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(view.dst_ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(view.dst_port, 18081);

        let target = transparent.get(view.src_port).unwrap();
        assert_eq!(target.original_dst.port(), 443);
        assert!(target.original_dst.is_ipv6());
        assert_eq!(target.original_dst.ip().to_string(), "2606:2800:220:1::1");
    }

    #[test]
    fn drops_quic_when_blocked() {
        let nat = Arc::new(Mutex::new(NatState::new(18081)));
        let transparent = TransparentMap::default();
        let dropped = AtomicU64::new(0);
        let mut packet = unsafe { WinDivertPacket::<NetworkLayer>::new(udp_packet()) };
        assert!(!run(
            &mut packet,
            &nat,
            &transparent,
            &[80, 443],
            true,
            &dropped
        ));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);

        let mut packet = unsafe { WinDivertPacket::<NetworkLayer>::new(udp_packet()) };
        assert!(run(
            &mut packet,
            &nat,
            &transparent,
            &[80, 443],
            false,
            &dropped
        ));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn extra_ports_are_intercepted_only_when_configured() {
        let nat = Arc::new(Mutex::new(NatState::new(18081)));
        let transparent = TransparentMap::default();
        let mut data = syn_packet();
        data[22..24].copy_from_slice(&8443u16.to_be_bytes());

        let mut packet = unsafe { WinDivertPacket::<NetworkLayer>::new(data.clone()) };
        assert!(run(
            &mut packet,
            &nat,
            &transparent,
            &[80, 443],
            false,
            &AtomicU64::new(0)
        ));
        assert!(!packet.address.loopback());
        assert!(nat.lock().unwrap().flows.is_empty());

        let mut packet = unsafe { WinDivertPacket::<NetworkLayer>::new(data) };
        assert!(run(
            &mut packet,
            &nat,
            &transparent,
            &[80, 443, 8443],
            false,
            &AtomicU64::new(0)
        ));
        assert!(packet.address.loopback());
        assert_eq!(nat.lock().unwrap().flows.len(), 1);
    }

    #[test]
    fn filter_avoids_parenthesized_negation() {
        let filter = build_filter(8080, &[80, 443, 8443], true);
        assert!(!filter.contains("!("));
        assert!(filter.contains("tcp.DstPort == 8443"));
        assert!(filter.contains("tcp.SrcPort < 30000"));
        assert!(filter.contains("tcp.SrcPort > 39999"));
        assert!(filter.contains("udp.DstPort == 443"));
        assert!(filter.contains("loopback and tcp and tcp.SrcPort == 8080"));
    }
}
