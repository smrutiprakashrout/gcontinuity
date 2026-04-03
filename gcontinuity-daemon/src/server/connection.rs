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
use crate::pairing::{DaemonEvent, PairingSession};
use crate::keepalive::KeepaliveTask;
use crate::identity::Identity;
use gcontinuity_common::{Packet, ConnectionState};

pub async fn handle(
    ws_stream: WebSocketStream<TlsStream<TcpStream>>,
    peer_addr: SocketAddr,
    store: Arc<PeerStore>,
    dbus_tx: broadcast::Sender<DaemonEvent>,
    identity: Arc<Identity>,
) {
    tracing::info!("Connection task started for {}", peer_addr);

    let (ws_sink, mut ws_stream_rx) = ws_stream.split();
    let ws_tx = Arc::new(Mutex::new(ws_sink));
    let last_pong = Arc::new(Mutex::new(Instant::now()));

    // Create a local mpsc channel for DaemonEvents so PairingSession matches its signature
    // Then forward it to the broadcase channel
    let (local_tx, mut local_rx) = mpsc::channel(32);
    let broadcast_tx = dbus_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = local_rx.recv().await {
            let _ = broadcast_tx.send(event);
        }
    });

    let mut session = PairingSession {
        state: ConnectionState::Idle,
        peer_info: None,
        store: store.clone(),
        dbus_tx: local_tx,
        ws_tx: ws_tx.clone(),
        last_pong: last_pong.clone(),
        identity: identity.clone(),
    };

    let (disconnect_tx, mut disconnect_rx) = oneshot::channel();
    let _keepalive_handle = KeepaliveTask::spawn(ws_tx.clone(), last_pong.clone(), disconnect_tx);

    loop {
        tokio::select! {
            msg_result = ws_stream_rx.next() => {
                let msg = match msg_result {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::error!("WebSocket error on {}: {}", peer_addr, e);
                        break;
                    }
                    None => {
                        tracing::info!("Connection closed by peer {}", peer_addr);
                        break;
                    }
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

    tracing::info!("Connection task ended for {}", peer_addr);
}
