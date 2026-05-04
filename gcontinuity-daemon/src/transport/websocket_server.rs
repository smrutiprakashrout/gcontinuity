//! Async TLS WebSocket server — one task per peer.
//!
//! FIXES:
//!   1. Trust check now uses PeerStore (sled DB) not config.paired_devices (TOML).
//!      config.paired_devices was always empty — trusted devices live in sled.
//!   2. NEW devices: perform_handshake blocks on PairingGate oneshot until
//!      GTK user clicks Accept or Reject. No more auto-accept for strangers.
//!   3. KNOWN devices: auto-trusted silently — no PairingRequested event fired.
//!      This fixes the dialog appearing on every auto-reconnect.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, Mutex, oneshot};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::store::PeerStore;
use crate::tls::TlsIdentity;
use crate::transport::peer::{PeerRegistry, PeerState};
use crate::transport::packet::Packet;
use crate::transport::FeatureEvent;
use gcontinuity_common::Packet as CommonPacket;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum TransportEvent {
    DeviceConnected    { device_id: String, name: String, addr: String },
    DeviceDisconnected { device_id: String },
    PairingRequested   { device_id: String, name: String, fingerprint: String },
    PairingAccepted    { device_id: String },
    PairingRejected    { device_id: String },
    PacketReceived     { device_id: String, packet: Packet },
    FileProgress       { file_id: String, bytes_done: u64, total: u64 },
}

// ── PairingGate ───────────────────────────────────────────────────────────────
// Delivers GTK user accept/reject decision to the waiting handshake task.

#[derive(Clone, Default)]
pub struct PairingGate {
    inner: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
}

impl PairingGate {
    pub fn new() -> Self { Self::default() }

    pub async fn register(&self, device_id: &str) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.inner.lock().await.insert(device_id.to_string(), tx);
        rx
    }

    pub async fn resolve(&self, device_id: &str, accepted: bool) -> bool {
        if let Some(tx) = self.inner.lock().await.remove(device_id) {
            let _ = tx.send(accepted);
            true
        } else {
            false
        }
    }

    pub async fn remove(&self, device_id: &str) {
        self.inner.lock().await.remove(device_id);
    }
}

// ── Context ───────────────────────────────────────────────────────────────────

struct PeerCtx {
    peer_addr:        SocketAddr,
    acceptor:         TlsAcceptor,
    config:           Arc<Config>,
    store:            Arc<PeerStore>,
    pairing_gate:     PairingGate,
    device_id:        String,
    tls_fingerprint:  String,
    registry:         Arc<PeerRegistry>,
    event_tx:         broadcast::Sender<TransportEvent>,
    feature_tx:       mpsc::Sender<FeatureEvent>,
    cancel:           CancellationToken,
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run_server(
    config:          Arc<Config>,
    tls:             Arc<TlsIdentity>,
    store:           Arc<PeerStore>,
    pairing_gate:    PairingGate,
    device_id:       String,
    tls_fingerprint: String,
    registry:        Arc<PeerRegistry>,
    event_tx:        broadcast::Sender<TransportEvent>,
    feature_tx:      mpsc::Sender<FeatureEvent>,
    cancel:          CancellationToken,
) -> Result<()> {
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind WebSocket server to {addr}"))?;

    tracing::info!("Listening on {addr}");
    let acceptor = TlsAcceptor::from(tls.server_config.clone());

    loop {
        tokio::select! {
            accept_res = listener.accept() => {
                let (tcp, peer_addr) = accept_res.context("accept() failed")?;
                let ctx = PeerCtx {
                    peer_addr,
                    acceptor:        acceptor.clone(),
                    config:          config.clone(),
                    store:           store.clone(),
                    pairing_gate:    pairing_gate.clone(),
                    device_id:       device_id.clone(),
                    tls_fingerprint: tls_fingerprint.clone(),
                    registry:        registry.clone(),
                    event_tx:        event_tx.clone(),
                    feature_tx:      feature_tx.clone(),
                    cancel:          cancel.clone(),
                };
                tokio::spawn(async move {
                    if let Err(e) = handle_peer(tcp, ctx).await {
                        tracing::warn!(%peer_addr, "Peer task error: {e:#}");
                    }
                });
            }
            _ = cancel.cancelled() => {
                tracing::info!("Shutdown: closing WebSocket listener");
                break;
            }
        }
    }

    registry.broadcast(Packet::Disconnect).await;
    tracing::info!("Sent Disconnect to all peers; server stopped");
    Ok(())
}

// ── Per-peer handler ──────────────────────────────────────────────────────────

async fn handle_peer(tcp: tokio::net::TcpStream, ctx: PeerCtx) -> Result<()> {
    let PeerCtx {
        peer_addr, acceptor, config, store, pairing_gate,
        device_id, tls_fingerprint, registry, event_tx, feature_tx, cancel,
    } = ctx;

    let tls_stream = acceptor.accept(tcp).await
        .with_context(|| format!("TLS handshake failed with {peer_addr}"))?;

    let ws_stream = tokio_tungstenite::accept_async(tls_stream).await
        .with_context(|| format!("WebSocket upgrade failed for {peer_addr}"))?;

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    let handshake = tokio::time::timeout(
        Duration::from_secs(30),
        perform_handshake(
            &mut ws_rx, &mut ws_tx,
            &config, &store, &pairing_gate,
            &device_id, &tls_fingerprint,
            &event_tx, peer_addr,
        ),
    )
    .await
    .context("Handshake timeout (30 s)")??;

    let (peer_device_id, name, session_token) = handshake;
    tracing::info!(%peer_addr, %peer_device_id, %name, "Handshake complete");

    let (peer_tx, mut peer_rx) = mpsc::channel::<Packet>(64);
    let mut state = PeerState::new(peer_device_id.clone(), name.clone(), peer_tx);
    state.handle.session_token = session_token;
    registry.register(state).await;

    let _ = event_tx.send(TransportEvent::DeviceConnected {
        device_id: peer_device_id.clone(),
        name:      name.clone(),
        addr:      peer_addr.to_string(),
    });

    ws_tx.send(Message::Text(Packet::Ack.to_json())).await
        .context("Failed to send Ack")?;

    let mut last_rx      = tokio::time::Instant::now();
    let mut ping_pending = false;

    let result: Result<()> = loop {
        tokio::select! {
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(json))) => {
                        last_rx      = tokio::time::Instant::now();
                        ping_pending = false;
                        match Packet::from_json(&json) {
                            Ok(pkt) => {
                                registry.inc_received(&peer_device_id).await;
                                let _ = event_tx.send(TransportEvent::PacketReceived {
                                    device_id: peer_device_id.clone(),
                                    packet:    pkt.clone(),
                                });
                                crate::transport::route_packet(
                                    pkt, &peer_device_id, &registry,
                                    &feature_tx, &event_tx,
                                ).await;
                            }
                            Err(e) => tracing::warn!(%peer_addr, "Bad packet: {e}"),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::info!(%peer_addr, "Peer closed connection");
                        break Ok(());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Err(anyhow::anyhow!("WS error: {e}")),
                }
            }
            Some(pkt) = peer_rx.recv() => {
                if let Err(e) = ws_tx.send(Message::Text(pkt.to_json())).await {
                    break Err(anyhow::anyhow!("Write failed: {e}"));
                }
                registry.inc_sent(&peer_device_id).await;
            }
            _ = tokio::time::sleep(Duration::from_secs(30)),
                if last_rx.elapsed() >= Duration::from_secs(30) && !ping_pending => {
                ping_pending = true;
                if let Err(e) = ws_tx.send(Message::Text(Packet::Ping.to_json())).await {
                    break Err(anyhow::anyhow!("Ping send failed: {e}"));
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(10)), if ping_pending => {
                tracing::warn!(%peer_device_id, "Pong timeout — closing dead connection");
                break Ok(());
            }
            _ = cancel.cancelled() => {
                let _ = ws_tx.send(Message::Text(Packet::Disconnect.to_json())).await;
                break Ok(());
            }
        }
    };

    registry.remove(&peer_device_id).await;
    let _ = event_tx.send(TransportEvent::DeviceDisconnected {
        device_id: peer_device_id,
    });
    result
}

// ── Handshake ─────────────────────────────────────────────────────────────────

async fn perform_handshake(
    ws_rx: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    ws_tx: &mut (impl SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    config:          &Config,
    store:           &PeerStore,
    pairing_gate:    &PairingGate,
    linux_device_id: &str,
    tls_fingerprint: &str,
    event_tx:        &broadcast::Sender<TransportEvent>,
    peer_addr:       SocketAddr,
) -> Result<(String, String, String)> {

    // Step 1: receive Hello
    let (peer_device_id, name) = loop {
        let json = match ws_rx.next().await {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(_)) => continue,
            Some(Err(e)) => anyhow::bail!("WS error waiting for Hello: {e}"),
            None => anyhow::bail!("Connection closed before Hello"),
        };
        match CommonPacket::from_json(&json) {
            Ok(CommonPacket::Hello { device_id, name, .. }) => {
                tracing::info!(%peer_addr, %device_id, %name, "Hello received");
                break (device_id, name);
            }
            Ok(CommonPacket::Disconnect) => anyhow::bail!("Disconnected before Hello"),
            Ok(_) | Err(_) => continue,
        }
    };

    // Step 2: send Linux Hello reply
    ws_tx.send(Message::Text(CommonPacket::Hello {
        device_id: linux_device_id.to_string(),
        name:      config.device_name.clone(),
        version:   1,
    }.to_json())).await.context("Failed to send Linux Hello")?;

    // Step 3: receive PairRequest
    let fingerprint = loop {
        let json = match ws_rx.next().await {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(_)) => continue,
            Some(Err(e)) => anyhow::bail!("WS error waiting for PairRequest: {e}"),
            None => anyhow::bail!("Connection closed before PairRequest"),
        };
        match CommonPacket::from_json(&json) {
            Ok(CommonPacket::PairRequest { fingerprint, .. }) => {
                tracing::info!(%peer_addr, %peer_device_id, "PairRequest received");
                break fingerprint;
            }
            Ok(CommonPacket::Disconnect) => anyhow::bail!("Disconnected before PairRequest"),
            Ok(_) | Err(_) => continue,
        }
    };

    // Step 4: trust decision using PeerStore (sled DB — the real trusted store)
    if store.is_trusted(&peer_device_id) {
        let stored_fp = store.get_fingerprint(&peer_device_id).unwrap_or_default();
        if stored_fp == fingerprint {
            // FIX: Known trusted device — auto-accept, NO PairingRequested event.
            // This prevents the GTK dialog appearing on every auto-reconnect.
            tracing::info!(%peer_device_id, "Known trusted device — auto-accepted silently");
            ws_tx.send(Message::Text(
                CommonPacket::PairAccept { fingerprint: tls_fingerprint.to_string() }.to_json()
            )).await.context("Failed to send PairAccept")?;
        } else {
            tracing::warn!(%peer_device_id, "Fingerprint mismatch — rejecting (possible MITM)");
            let _ = ws_tx.send(Message::Text(
                CommonPacket::PairReject { reason: "fingerprint_changed".into() }.to_json()
            )).await;
            let _ = event_tx.send(TransportEvent::PairingRejected {
                device_id: peer_device_id.clone(),
            });
            anyhow::bail!("Fingerprint mismatch for {peer_device_id}");
        }
    } else {
        // FIX: NEW device — fire PairingRequested and BLOCK until GTK decides.
        // Old code auto-accepted here which made the GTK buttons useless.
        tracing::info!(%peer_device_id, %name, "New device — waiting for user decision (120s timeout)");
        let rx = pairing_gate.register(&peer_device_id).await;
        let _ = event_tx.send(TransportEvent::PairingRequested {
            device_id:   peer_device_id.clone(),
            name:        name.clone(),
            fingerprint: fingerprint.clone(),
        });

        match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(true)) => {
                let device = gcontinuity_common::DeviceInfo {
                    device_id:   peer_device_id.clone(),
                    name:        name.clone(),
                    fingerprint: fingerprint.clone(),
                    version:     1,
                };
                store.store_device(&device)?;
                ws_tx.send(Message::Text(
                    CommonPacket::PairAccept { fingerprint: tls_fingerprint.to_string() }.to_json()
                )).await.context("Failed to send PairAccept")?;
                let _ = event_tx.send(TransportEvent::PairingAccepted {
                    device_id: peer_device_id.clone(),
                });
                tracing::info!(%peer_device_id, "Pairing accepted by user");
            }
            Ok(Ok(false)) | Ok(Err(_)) => {
                let _ = ws_tx.send(Message::Text(
                    CommonPacket::PairReject { reason: "user_rejected".into() }.to_json()
                )).await;
                let _ = event_tx.send(TransportEvent::PairingRejected {
                    device_id: peer_device_id.clone(),
                });
                anyhow::bail!("Pairing rejected by user for {peer_device_id}");
            }
            Err(_) => {
                pairing_gate.remove(&peer_device_id).await;
                let _ = ws_tx.send(Message::Text(
                    CommonPacket::PairReject { reason: "timeout".into() }.to_json()
                )).await;
                anyhow::bail!("Pairing timeout for {peer_device_id}");
            }
        }
    }

    let session_token = uuid::Uuid::new_v4().to_string();
    Ok((peer_device_id, name, session_token))
}
