// gcontinuity-daemon/src/pairing/mod.rs
//
// CHANGES FROM OLD VERSION:
//   1. `Packet::Hello` no longer has `fingerprint` field — removed.
//      Fingerprint exchange happens via PairRequest/PairAccept.
//   2. `Packet::Ping { timestamp_ms }` → `Packet::Ping` (bare, no fields).
//   3. `Packet::Pong { timestamp_ms }` → `Packet::Pong` (bare, no fields).
//   4. `Packet::Disconnect { reason }` → `Packet::Disconnect` (bare, no fields).
//   5. `hello.to_json()` was `to_json()?` (Result) — now `to_json()` (String).
//      Updated all call sites accordingly.
//   6. Fingerprint is stored and checked from the PairRequest packet, not Hello.
//   7. Linux → Android Hello reply now sends Linux's own fingerprint via
//      a follow-up PairAccept after identity check, NOT embedded in Hello.

#![allow(dead_code)] // Phase 1 — reactivated in Phase 3

use anyhow::Result;
use gcontinuity_common::{ConnectionState, DeviceInfo, Packet};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::Message;
use futures_util::stream::SplitSink;
use tokio_tungstenite::WebSocketStream;
use tokio_rustls::server::TlsStream;
use tokio::net::TcpStream;
use futures_util::SinkExt;
use std::time::Instant;
use tokio::sync::{Mutex, oneshot};
use std::collections::HashMap;
use crate::store::PeerStore;
use crate::identity::Identity;

// ── D-Bus event types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DaemonEvent {
    PairingRequested   { device: DeviceInfo },
    PairingCompleted   { device_id: String },
    PairingRejected    { device_id: String },
    DeviceConnected    { device: DeviceInfo },
    DeviceDisconnected { device_id: String },
}

// ── PairingGate — one-shot user decision per device_id ──────────────────────

#[derive(Default, Clone)]
pub struct PairingGate {
    inner: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
}

impl PairingGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pending pairing decision; returns a receiver that resolves
    /// to `true` (accepted) or `false` (rejected).
    pub async fn register(&self, device_id: &str) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.inner.lock().await.insert(device_id.to_string(), tx);
        rx
    }

    /// Resolve a pending decision.  Returns `true` if the channel was found.
    pub async fn resolve(&self, device_id: &str, accepted: bool) -> bool {
        if let Some(tx) = self.inner.lock().await.remove(device_id) {
            let _ = tx.send(accepted);
            true
        } else {
            false
        }
    }

    /// Discard a pending decision without resolving it (e.g. timeout cleanup).
    pub async fn remove(&self, device_id: &str) {
        self.inner.lock().await.remove(device_id);
    }
}

// ── PairingSession — per-connection state machine ───────────────────────────

pub struct PairingSession {
    pub state:            ConnectionState,
    pub peer_info:        Option<DeviceInfo>,
    pub store:            Arc<PeerStore>,
    pub dbus_tx:          tokio::sync::mpsc::Sender<DaemonEvent>,
    pub ws_tx:            Arc<Mutex<SplitSink<WebSocketStream<TlsStream<TcpStream>>, Message>>>,
    pub last_pong:        Arc<Mutex<Instant>>,
    pub identity:         Arc<Identity>,
    pub gate:             PairingGate,
    /// Shared with DaemonInterface — the single source of truth for which
    /// device is currently live.  Written here, read by get_connected_device().
    pub connected_device: Arc<Mutex<Option<DeviceInfo>>>,
}

impl PairingSession {
    // ── Internal helpers ─────────────────────────────────────────────────────

    /// Send a raw JSON string to the peer.
    async fn send_json(&self, json: String) {
        let mut sink = self.ws_tx.lock().await;
        let _ = sink.send(Message::Text(json)).await;
    }

    /// Mark a device as connected in the shared Arc and fire D-Bus event.
    async fn set_connected(&mut self, info: DeviceInfo) {
        *self.connected_device.lock().await = Some(info.clone());
        let _ = self.dbus_tx.send(DaemonEvent::DeviceConnected { device: info }).await;
    }

    /// Clear the shared Arc and fire D-Bus disconnect event.
    async fn set_disconnected(&mut self, device_id: String) {
        *self.connected_device.lock().await = None;
        let _ = self.dbus_tx
            .send(DaemonEvent::DeviceDisconnected { device_id })
            .await;
    }

    // ── Packet dispatch ──────────────────────────────────────────────────────

    pub async fn handle_packet(&mut self, packet: Packet) -> Result<()> {
        match packet {
            // ── Step 1: Android sends Hello ──────────────────────────────────
            //
            // Android's Hello carries {device_id, name, version} only — no
            // fingerprint.  Linux replies with its own Hello, then waits for
            // Android to send a PairRequest (new device) or for the auto-trust
            // path (known device, checked after PairRequest is received).
            Packet::Hello { device_id, name, version } => {
                tracing::info!(
                    "Hello from {} ({}) protocol v{}",
                    name,
                    device_id,
                    version
                );

                // Reply with Linux's own Hello so Android knows who it's
                // talking to before we decide on trust.
                let reply = Packet::Hello {
                    device_id: self.identity.device_id.clone(),
                    name:      self.identity.name.clone(),
                    version:   1,
                };
                self.send_json(reply.to_json()).await;

                let info = DeviceInfo {
                    device_id: device_id.clone(),
                    name:      name.clone(),
                    // fingerprint will be filled in from PairRequest below
                    fingerprint: String::new(),
                    version,
                };
                self.peer_info = Some(info);
                self.state = ConnectionState::AwaitingPair;
            }

            // ── Step 2: Android sends PairRequest ────────────────────────────
            //
            // Now we have the fingerprint.  Check if this is a known device
            // (auto-trust) or a new one (show UI).
            Packet::PairRequest { device_id, name, fingerprint } => {
                tracing::info!("PairRequest from {} ({})", name, device_id);

                // Update peer_info with the real fingerprint.
                let info = DeviceInfo {
                    device_id:   device_id.clone(),
                    name:        name.clone(),
                    fingerprint: fingerprint.clone(),
                    version:     self.peer_info.as_ref().map(|i| i.version).unwrap_or(1),
                };
                self.peer_info = Some(info.clone());

                if let Some(stored_fp) = self.store.get_fingerprint(&device_id) {
                    if stored_fp == fingerprint {
                        // Known device — auto-trust immediately.
                        self.store.store_device(&info)?;
                        self.state = ConnectionState::PairedConnected;
                        self.send_json(
                            Packet::PairAccept {
                                fingerprint: self.identity.fingerprint.clone(),
                            }
                            .to_json(),
                        )
                        .await;
                        let _ = self.dbus_tx
                            .send(DaemonEvent::PairingCompleted {
                                device_id: info.device_id.clone(),
                            })
                            .await;
                        self.set_connected(info).await;
                        tracing::info!("Auto-trusted device: {}", name);
                    } else {
                        // Fingerprint changed — reject as a security measure.
                        self.state = ConnectionState::Disconnected;
                        self.send_json(
                            Packet::PairReject {
                                reason: "fingerprint_changed".into(),
                            }
                            .to_json(),
                        )
                        .await;
                        let _ = self.dbus_tx
                            .send(DaemonEvent::PairingRejected { device_id })
                            .await;
                        return Err(anyhow::anyhow!(
                            "Fingerprint mismatch for {} — possible MITM",
                            name
                        ));
                    }
                } else {
                    // Unknown device — wait for the user to accept or reject.
                    self.state = ConnectionState::AwaitingPair;
                    let rx = self.gate.register(&device_id).await;
                    let _ = self.dbus_tx
                        .send(DaemonEvent::PairingRequested { device: info.clone() })
                        .await;
                    tracing::info!("Awaiting user decision for {}", name);

                    match tokio::time::timeout(
                        std::time::Duration::from_secs(120),
                        rx,
                    )
                    .await
                    {
                        Ok(Ok(true)) => {
                            self.store.store_device(&info)?;
                            self.state = ConnectionState::PairedConnected;
                            self.send_json(
                                Packet::PairAccept {
                                    fingerprint: self.identity.fingerprint.clone(),
                                }
                                .to_json(),
                            )
                            .await;
                            let _ = self.dbus_tx
                                .send(DaemonEvent::PairingCompleted {
                                    device_id: info.device_id.clone(),
                                })
                                .await;
                            self.set_connected(info).await;
                            tracing::info!("User accepted pairing for {}", name);
                        }
                        Ok(Ok(false)) | Ok(Err(_)) => {
                            self.state = ConnectionState::Disconnected;
                            self.send_json(
                                Packet::PairReject {
                                    reason: "user_rejected".into(),
                                }
                                .to_json(),
                            )
                            .await;
                            let _ = self.dbus_tx
                                .send(DaemonEvent::PairingRejected { device_id })
                                .await;
                            return Err(anyhow::anyhow!("Pairing rejected by user"));
                        }
                        Err(_) => {
                            self.state = ConnectionState::Disconnected;
                            self.gate.remove(&device_id).await;
                            self.send_json(
                                Packet::PairReject {
                                    reason: "timeout".into(),
                                }
                                .to_json(),
                            )
                            .await;
                            tracing::warn!("Pairing timeout for {}", name);
                            return Err(anyhow::anyhow!("Pairing timed out"));
                        }
                    }
                }
            }

            // ── PairAccept from Android (Android-initiated flow) ──────────────
            Packet::PairAccept { fingerprint } => {
                if self.state == ConnectionState::AwaitingPair {
                    if let Some(mut info) = self.peer_info.clone() {
                        info.fingerprint = fingerprint;
                        self.store.store_device(&info)?;
                        self.state = ConnectionState::PairedConnected;
                        let _ = self.dbus_tx
                            .send(DaemonEvent::PairingCompleted {
                                device_id: info.device_id.clone(),
                            })
                            .await;
                        self.set_connected(info).await;
                    }
                }
            }

            // ── PairReject from Android ──────────────────────────────────────
            Packet::PairReject { reason } => {
                self.state = ConnectionState::Disconnected;
                tracing::warn!("Pairing rejected by Android: {}", reason);
                if let Some(info) = &self.peer_info {
                    let _ = self.dbus_tx
                        .send(DaemonEvent::PairingRejected {
                            device_id: info.device_id.clone(),
                        })
                        .await;
                }
                return Err(anyhow::anyhow!("Pairing rejected by Android: {}", reason));
            }

            // ── Keepalive ────────────────────────────────────────────────────
            // Ping and Pong are now bare (no timestamp_ms field).
            Packet::Ping => {
                self.send_json(Packet::Pong.to_json()).await;
            }

            Packet::Pong => {
                *self.last_pong.lock().await = Instant::now();
            }

            // ── Graceful disconnect ──────────────────────────────────────────
            // Disconnect is bare (no reason field) — reason is implicit.
            Packet::Disconnect => {
                self.state = ConnectionState::Disconnected;
                if let Some(info) = &self.peer_info {
                    self.gate.remove(&info.device_id).await;
                    self.set_disconnected(info.device_id.clone()).await;
                }
                tracing::info!("Graceful disconnect from peer");
                // Return an error to signal the connection loop to exit.
                return Err(anyhow::anyhow!("peer_disconnected"));
            }
        }
        Ok(())
    }

    // ── UI-triggered actions (called from D-Bus / GTK) ───────────────────────

    /// Called when the Linux user clicks "Accept" in the GTK pairing dialog.
    pub async fn accept_pairing(&mut self) -> Result<()> {
        if let Some(info) = self.peer_info.clone() {
            self.send_json(
                Packet::PairAccept {
                    fingerprint: self.identity.fingerprint.clone(),
                }
                .to_json(),
            )
            .await;
            self.store.store_device(&info)?;
            self.state = ConnectionState::PairedConnected;
            let _ = self.dbus_tx
                .send(DaemonEvent::PairingCompleted {
                    device_id: info.device_id.clone(),
                })
                .await;
            self.set_connected(info).await;
        }
        Ok(())
    }

    /// Called when the Linux user clicks "Reject" in the GTK pairing dialog.
    pub async fn reject_pairing(&mut self) -> Result<()> {
        if let Some(info) = self.peer_info.clone() {
            self.send_json(
                Packet::PairReject {
                    reason: "user_rejected".into(),
                }
                .to_json(),
            )
            .await;
            self.state = ConnectionState::Disconnected;
            self.gate.remove(&info.device_id).await;
            let _ = self.dbus_tx
                .send(DaemonEvent::PairingRejected {
                    device_id: info.device_id.clone(),
                })
                .await;
        }
        Ok(())
    }
}
