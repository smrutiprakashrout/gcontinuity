//! Per-peer state and the in-process peer registry.
#![allow(dead_code)] // Phase 3+ will use find_by_session, connected_at, inc_sent/received

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::transport::packet::Packet;

// ── Handle ───────────────────────────────────────────────────────────────────

/// Lightweight, cheaply-cloneable reference to a connected peer.
#[derive(Debug, Clone)]
pub struct PeerHandle {
    /// Stable UUID identifying the Android device.
    pub device_id: String,
    /// Human-readable device name.
    pub name: String,
    /// UUID v4 assigned at connection time; persisted for session-resume.
    pub session_token: String,
    /// Send packets to this peer via the per-peer writer task.
    pub tx: mpsc::Sender<Packet>,
}

// ── State ────────────────────────────────────────────────────────────────────

/// Full mutable state tracked for one connected peer.
pub struct PeerState {
    pub handle: PeerHandle,
    pub connected_at: Instant,
    pub packets_sent: u64,
    pub packets_received: u64,
}

impl PeerState {
    /// Create a fresh `PeerState` for a new (non-resumed) connection.
    pub fn new(device_id: String, name: String, tx: mpsc::Sender<Packet>) -> Self {
        Self {
            handle: PeerHandle {
                device_id,
                name,
                // A fresh UUID v4 token is assigned on every clean connect —
                // it is returned to Android in the Ack so it can resume later.
                session_token: Uuid::new_v4().to_string(),
                tx,
            },
            connected_at: Instant::now(),
            packets_sent: 0,
            packets_received: 0,
        }
    }
}

// ── Registry ─────────────────────────────────────────────────────────────────

/// Thread-safe registry of all currently-connected peers.
///
/// Uses `RwLock` rather than `Mutex` because reads (get_all, find_by_session)
/// dominate at runtime; the write lock is only taken on connect/disconnect.
#[derive(Clone, Default)]
pub struct PeerRegistry {
    /// Keyed by device_id for O(1) targeted sends.
    peers: Arc<RwLock<HashMap<String, PeerState>>>,
}

impl PeerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a newly connected peer.
    pub async fn register(&self, state: PeerState) {
        let id = state.handle.device_id.clone();
        self.peers.write().await.insert(id, state);
    }

    /// Remove a peer by device_id (called on disconnect / keepalive timeout).
    pub async fn remove(&self, device_id: &str) {
        self.peers.write().await.remove(device_id);
    }

    /// Return lightweight handles for all connected peers (used by D-Bus
    /// `GetConnectedDevices` and for broadcasting).
    pub async fn get_all(&self) -> Vec<PeerHandle> {
        self.peers
            .read()
            .await
            .values()
            .map(|s| s.handle.clone())
            .collect()
    }

    /// Look up a peer by its session token — used for `SessionResume`.
    pub async fn find_by_session(&self, token: &str) -> Option<PeerHandle> {
        self.peers
            .read()
            .await
            .values()
            .find(|s| s.handle.session_token == token)
            .map(|s| s.handle.clone())
    }

    /// Send a packet to one specific peer.  Returns `Err` if the peer is not
    /// connected or the channel is full/closed.
    pub async fn send_to(&self, device_id: &str, packet: Packet) -> Result<()> {
        let guard = self.peers.read().await;
        let state = guard
            .get(device_id)
            .with_context(|| format!("Device '{}' not connected", device_id))?;
        state
            .handle
            .tx
            .send(packet)
            .await
            .context("Peer send channel closed")?;
        Ok(())
    }

    /// Broadcast a packet to every connected peer.  Per-peer failures are
    /// logged but do not abort the broadcast (a slow/dead peer must not
    /// block others).
    pub async fn broadcast(&self, packet: Packet) {
        let handles = self.get_all().await;
        for handle in handles {
            if let Err(e) = handle.tx.send(packet.clone()).await {
                tracing::warn!(
                    device_id = %handle.device_id,
                    "Broadcast failed for peer: {}", e
                );
            }
        }
    }

    /// Increment sent counter for a peer (called by the writer task).
    pub async fn inc_sent(&self, device_id: &str) {
        if let Some(state) = self.peers.write().await.get_mut(device_id) {
            state.packets_sent += 1;
        }
    }

    /// Increment received counter for a peer (called by the reader loop).
    pub async fn inc_received(&self, device_id: &str) {
        if let Some(state) = self.peers.write().await.get_mut(device_id) {
            state.packets_received += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn make_state(id: &str) -> (PeerState, mpsc::Receiver<Packet>) {
        let (tx, rx) = mpsc::channel(8);
        (PeerState::new(id.into(), format!("{id}-name"), tx), rx)
    }

    #[tokio::test]
    async fn test_register_and_get_all() {
        let reg = PeerRegistry::new();
        let (s, _rx) = make_state("dev1");
        reg.register(s).await;
        let all = reg.get_all().await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].device_id, "dev1");
    }

    #[tokio::test]
    async fn test_remove() {
        let reg = PeerRegistry::new();
        let (s, _rx) = make_state("dev1");
        reg.register(s).await;
        reg.remove("dev1").await;
        assert!(reg.get_all().await.is_empty());
    }

    #[tokio::test]
    async fn test_find_by_session() {
        let reg = PeerRegistry::new();
        let (s, _rx) = make_state("dev1");
        let token = s.handle.session_token.clone();
        reg.register(s).await;
        let found = reg.find_by_session(&token).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().device_id, "dev1");
    }

    #[tokio::test]
    async fn test_send_to_delivers() {
        let reg = PeerRegistry::new();
        let (s, mut rx) = make_state("dev1");
        reg.register(s).await;
        reg.send_to("dev1", Packet::Ping).await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), Packet::Ping);
    }

    #[tokio::test]
    async fn test_send_to_unknown_errors() {
        let reg = PeerRegistry::new();
        assert!(reg.send_to("ghost", Packet::Ping).await.is_err());
    }

    #[tokio::test]
    async fn test_broadcast_reaches_all() {
        let reg = PeerRegistry::new();
        let (s1, mut rx1) = make_state("dev1");
        let (s2, mut rx2) = make_state("dev2");
        reg.register(s1).await;
        reg.register(s2).await;
        reg.broadcast(Packet::Pong).await;
        assert_eq!(rx1.recv().await.unwrap(), Packet::Pong);
        assert_eq!(rx2.recv().await.unwrap(), Packet::Pong);
    }
}
