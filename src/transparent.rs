use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy)]
pub struct TransparentTarget {
    pub client: SocketAddr,
    pub original_dst: SocketAddr,
}

#[derive(Default)]
pub struct TransparentMap {
    entries: Mutex<HashMap<u16, TransparentTarget>>,
}

impl TransparentMap {
    pub fn insert(&self, relay_src_port: u16, target: TransparentTarget) {
        if let Ok(mut map) = self.entries.lock() {
            if map.len() > 8192 {
                map.clear();
            }
            map.insert(relay_src_port, target);
        }
    }

    pub fn take(&self, relay_src_port: u16) -> Option<TransparentTarget> {
        self.entries
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&relay_src_port))
    }

    pub fn get(&self, relay_src_port: u16) -> Option<TransparentTarget> {
        self.entries
            .lock()
            .ok()
            .and_then(|map| map.get(&relay_src_port).copied())
    }

    pub fn remove_client_port(&self, client_port: u16) {
        if let Ok(mut map) = self.entries.lock() {
            map.retain(|_, v| v.client.port() != client_port);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
