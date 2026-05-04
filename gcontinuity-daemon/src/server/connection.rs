#![allow(dead_code)] // Phase 1 — reactivated in Phase 3
use futures_util::StreamExt;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot, Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tokio_rustls::server::TlsStream;
use tokio::net::TcpStream;
use std::time::Instant;

use crate::store::PeerStore;
use crate::pairing::{DaemonEvent, PairingGate, PairingSession};
use crate::keepalive::KeepaliveTask;
use crate::identity::Identity;
use gcontinuity_common::{ConnectionState, DeviceInfo, Packet};

pub async fn handle(
    ws_stream: WebSocketStream<TlsStream<TcpStream>>,
    peer_addr: SocketAddr,
    store: Arc<PeerStore>,
    dbus_tx: broadcast::Sender<DaemonEvent>,
    identity: Arc<Identity>,
    gate: PairingGate,
    connected_device: Arc<Mutex<Option<DeviceInfo>>>,
) {
    tracing::info!("Connection task started for {}", peer_addr);

    let (ws_sink, mut ws_stream_rx) = ws_stream.split();
    let ws_tx    = Arc::new(Mutex::new(ws_sink));
    let last_pong = Arc::new(Mutex::new(Instant::now()));

    let (local_tx, mut local_rx) = mpsc::channel(32);
    let broadcast_tx = dbus_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = local_rx.recv().await {
            let _ = broadcast_tx.send(event);
        }
    });

    let mut session = PairingSession {
        state:            ConnectionState::Idle,
        peer_info:        None,
        store:            store.clone(),
        dbus_tx:          local_tx,
        ws_tx:            ws_tx.clone(),
        last_pong:        last_pong.clone(),
        identity:         identity.clone(),
        gate,
        connected_device, // <── injected, written on connect/disconnect
    };

    let (disconnect_tx, mut disconnect_rx) = oneshot::channel();
    let _keepalive_handle = KeepaliveTask::spawn(ws_tx.clone(), last_pong.clone(), disconnect_tx);

    loop {
        tokio::select! {
            msg_result = ws_stream_rx.next() => {
                let msg = match msg_result {
                    Some(Ok(m))  => m,
                    Some(Err(e)) => { tracing::error!("WebSocket error on {}: {}", peer_addr, e); break; }
                    None         => { tracing::info!("Connection closed by peer {}", peer_addr); break; }
                };
                match msg {
                    Message::Text(json) => {
                        tracing::debug!("RX: {}", json);
                        if let Ok(packet) = Packet::from_json(&json) {
                            if let Err(e) = session.handle_packet(packet).await {
                                tracing::error!("Error handling packet: {}", e);
                                break;
                            }
                        }
                    }
                    Message::Close(_) => {
                        tracing::info!("Connection closed gracefully by peer {}", peer_addr);
                        break;
                    }
                    _ => {}
                }
            }
            _ = &mut disconnect_rx => {
                tracing::warn!("Disconnect signal received, closing task for {}", peer_addr);
                break;
            }
        }
    }

    // Ensure connected_device is cleared if the task exits for any reason
    // (e.g. TCP drop, keepalive timeout) without a clean Disconnect packet.
    if let Some(info) = &session.peer_info {
        let mut guard = session.connected_device.lock().await;
        if guard.as_ref().map(|d| &d.device_id) == Some(&info.device_id) {
            *guard = None;
            tracing::info!("Cleared connected_device on unclean exit for {}", info.device_id);
            // Fire DeviceDisconnected so GTK navigates back to waiting page.
            let _ = session.dbus_tx.send(DaemonEvent::DeviceDisconnected {
                device_id: info.device_id.clone(),
            }).await;
        }
    }

    tracing::info!("Connection task ended for {}", peer_addr);
}
