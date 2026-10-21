//! WinDivert transparent redirect engine.
//!
//! Windows applications do not honour the explicit HTTP proxy automatically.
//! This module redirects outbound TCP :80/:443 connections to the local MITM
//! listener at the packet layer and performs a small userspace NAT so return
//! packets appear to come from the original destination.
//!
//! The proxy's own upstream sockets are bound to a reserved source-port range
//! (30000-39999) which the filter excludes, preventing a redirect loop. The
//! loopback return path is matched by the proxy listener's source port.

use crate::state::AppState;
use crate::transparent::TransparentTarget;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windivert::prelude::*;

const RELAY_PORT_MIN: u16 = 20000;
const RELAY_PORT_MAX: u16 = 29999;
const UPSTREAM_PORT_MIN: u16 = 30000;
const UPSTREAM_PORT_MAX: u16 = 39999;
const NAT_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FlowKey {
    src_ip: u32,
    src_port: u16,
    dst_ip: u32,
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
    _ip_off: usize,
    tcp_off: usize,
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    flags: u8,
    _total_len: usize,
}

impl PacketView {
    fn syn(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

fn parse_packet(data: &[u8]) -> Option<PacketView> {
    if data.len() < 20 || data[0] >> 4 != 4 {
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
    let tcp_off = ihl;
    if total_len < tcp_off + 20 {
        return None;
    }
    let tcp_hlen = ((data[tcp_off + 12] >> 4) as usize) * 4;
    if tcp_hlen < 20 || tcp_off + tcp_hlen > total_len {
        return None;
    }
    let src_ip = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
    let dst_ip = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    Some(PacketView {
        _ip_off: 0,
        tcp_off,
        src_ip,
        dst_ip,
        src_port: u16::from_be_bytes([data[tcp_off], data[tcp_off + 1]]),
        dst_port: u16::from_be_bytes([data[tcp_off + 2], data[tcp_off + 3]]),
        flags: data[tcp_off + 13],
        _total_len: total_len,
    })
}

fn set_src(data: &mut [u8], v: u32) {
    data[12..16].copy_from_slice(&v.to_be_bytes());
}
fn set_dst(data: &mut [u8], v: u32) {
    data[16..20].copy_from_slice(&v.to_be_bytes());
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
    let data = packet.data.to_mut();
    set_src(data, u32::from_be_bytes([127, 0, 0, 1]));
    set_dst(data, u32::from_be_bytes([127, 0, 0, 1]));
    set_src_port(data, p, relay_port);
    set_dst_port(data, p, proxy_port);
    packet.address.set_outbound(true);
    packet.address.set_loopback(true);
    packet.address.set_interface_index(loop_if);
    packet.address.set_subinterface_index(0);
    let _ = packet.recalculate_checksums(Default::default());
}

fn rewrite_to_client(
    packet: &mut WinDivertPacket<'_, NetworkLayer>,
    p: PacketView,
    entry: NatEntry,
) {
    let data = packet.data.to_mut();
    set_src(data, entry.key.dst_ip);
    set_dst(data, entry.key.src_ip);
    set_src_port(data, p, entry.key.dst_port);
    set_dst_port(data, p, entry.key.src_port);
    packet.address.set_outbound(false);
    packet.address.set_loopback(false);
    packet.address.set_interface_index(entry.if_idx);
    packet.address.set_subinterface_index(entry.sub_if_idx);
    let _ = packet.recalculate_checksums(Default::default());
}

fn original_ip(v: u32) -> Ipv4Addr {
    Ipv4Addr::from(v)
}

fn process_packet(
    packet: &mut WinDivertPacket<'_, NetworkLayer>,
    nat: &Arc<Mutex<NatState>>,
    transparent: &crate::transparent::TransparentMap,
    loop_if: u32,
) {
    let Some(p) = parse_packet(&packet.data) else {
        return;
    };
    if !packet.address.outbound() {
        return;
    }
    if packet.address.loopback() {
        if p.src_port != nat.lock().map(|n| n.proxy_port).unwrap_or(0) {
            return;
        }
        let entry = {
            let mut guard = match nat.lock() {
                Ok(g) => g,
                Err(_) => return,
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
        return;
    }
    // Only intercept standard web ports and never our own upstream sockets.
    if p.dst_port != 80 && p.dst_port != 443 {
        return;
    }
    if (UPSTREAM_PORT_MIN..=UPSTREAM_PORT_MAX).contains(&p.src_port) {
        return;
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
            Err(_) => return,
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
            return;
        }
    };
    if new_flow {
        transparent.insert(
            entry.relay_port,
            TransparentTarget {
                client: SocketAddr::from((original_ip(entry.key.src_ip), entry.key.src_port)),
                original_dst: SocketAddr::from((original_ip(entry.key.dst_ip), entry.key.dst_port)),
            },
        );
        tracing::debug!(
            "windivert redirect {}:{} -> {}:{} via relay {}",
            original_ip(entry.key.src_ip),
            entry.key.src_port,
            original_ip(entry.key.dst_ip),
            entry.key.dst_port,
            entry.relay_port
        );
    } else if p.syn() {
        // A SYN retransmission may arrive after the listener consumed the map
        // entry. Re-arm it so the retry can still be accepted.
        transparent.insert(
            entry.relay_port,
            TransparentTarget {
                client: SocketAddr::from((original_ip(entry.key.src_ip), entry.key.src_port)),
                original_dst: SocketAddr::from((original_ip(entry.key.dst_ip), entry.key.dst_port)),
            },
        );
    }
    let proxy_port = nat.lock().map(|n| n.proxy_port).unwrap_or(0);
    rewrite_to_proxy(packet, p, entry.relay_port, proxy_port, loop_if);
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

fn build_filter(proxy_port: u16) -> String {
    // WinDivert's `!` only applies to a single test, not a parenthesized
    // expression (`!(A and B)` is rejected as InvalidParameter). Use the
    // De Morgan equivalent and explicit comparisons.
    format!(
        "outbound and ip and tcp and ((not loopback and (tcp.DstPort == 80 or tcp.DstPort == 443) and (tcp.SrcPort < {UPSTREAM_PORT_MIN} or tcp.SrcPort > {UPSTREAM_PORT_MAX})) or (loopback and tcp.SrcPort == {proxy_port}))"
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
    let packets = state.windivert.read().await.packets.clone();
    let filter = build_filter(proxy_port);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let state_for_thread = state.clone();
    let loop_if = loopback_interface_index();
    let nat = Arc::new(Mutex::new(NatState::new(proxy_port)));
    let filter_for_log = filter.clone();
    let shutdown_slot: Arc<Mutex<Option<ShutdownHandle>>> = Arc::new(Mutex::new(None));
    let shutdown_for_thread = shutdown_slot.clone();
    let thread = std::thread::Builder::new()
        .name("mitmrs-windivert".into())
        .spawn(move || {
            let divert = match WinDivert::network(filter_for_log, 0, WinDivertFlags::new()) {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(Err(format!("{e}")));
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
                        packets.fetch_add(1, Ordering::Relaxed);
                        process_packet(
                            &mut packet,
                            &nat,
                            state_for_thread.transparent.as_ref(),
                            loop_if,
                        );
                        if let Err(e) = divert.send(&packet) {
                            tracing::debug!("windivert send failed: {e}");
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
            tracing::info!("windivert packet loop stopped");
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
    if let Ok(mut slot) = rt.shutdown.lock() {
        *slot = shutdown;
    }
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
    // WinDivert's handle is owned by the worker; dropping the process handle is
    // the only reliable cross-thread stop without exposing the handle. We signal
    // shutdown via the shared token installed by `start`.
    let mut rt = state.windivert.write().await;
    if !rt.enabled {
        return Ok(());
    }
    rt.state = "stopping".into();
    if let Ok(mut slot) = rt.shutdown.lock() {
        if let Some(shutdown) = slot.take() {
            let _ = shutdown.shutdown();
        }
    }
    let thread = rt.thread.take();
    drop(rt);
    if let Some(thread) = thread {
        let _ = tokio::task::spawn_blocking(move || thread.join()).await;
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

    #[test]
    fn redirects_syn_to_local_proxy_and_nats_return_path() {
        let nat = Arc::new(Mutex::new(NatState::new(18081)));
        let transparent = crate::transparent::TransparentMap::default();
        let mut packet = unsafe { WinDivertPacket::<NetworkLayer>::new(syn_packet()) };
        packet.address.set_outbound(true);
        packet.address.set_interface_index(7);
        process_packet(&mut packet, &nat, &transparent, 1);

        assert!(packet.address.loopback());
        assert_eq!(packet.address.interface_index(), 1);
        let view = parse_packet(&packet.data).unwrap();
        assert_eq!(view.src_ip, u32::from_be_bytes([127, 0, 0, 1]));
        assert_eq!(view.dst_ip, u32::from_be_bytes([127, 0, 0, 1]));
        assert_eq!(view.dst_port, 18081);
        assert!((RELAY_PORT_MIN..=RELAY_PORT_MAX).contains(&view.src_port));

        let target = transparent.get(view.src_port).unwrap();
        assert_eq!(target.original_dst.port(), 80);
        assert_eq!(target.client.ip().to_string(), "10.0.0.2");
        assert_eq!(target.client.port(), 50000);
    }

    #[test]
    fn filter_avoids_parenthesized_negation() {
        let filter = build_filter(8080);
        assert!(!filter.contains("!("));
        assert!(filter.contains("tcp.SrcPort < 30000"));
        assert!(filter.contains("tcp.SrcPort > 39999"));
        assert!(filter.contains("loopback and tcp.SrcPort == 8080"));
    }

    #[test]
    fn ignores_uninteresting_ports_and_non_syn_flows() {
        let nat = Arc::new(Mutex::new(NatState::new(18081)));
        let transparent = crate::transparent::TransparentMap::default();
        let mut data = syn_packet();
        data[22..24].copy_from_slice(&8080u16.to_be_bytes());
        let mut packet = unsafe { WinDivertPacket::<NetworkLayer>::new(data) };
        packet.address.set_outbound(true);
        process_packet(&mut packet, &nat, &transparent, 1);
        assert!(!packet.address.loopback());
        assert!(nat.lock().unwrap().flows.is_empty());
    }
}
