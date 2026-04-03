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
use tokio::sync::Mutex;
use crate::store::PeerStore;
use crate::identity::Identity;

#[derive(Debug, Clone)]
pub enum DaemonEvent {
    PairingRequested { device: DeviceInfo },
    PairingCompleted { device_id: String },
    PairingRejected  { device_id: String },
    DeviceConnected  { device: DeviceInfo },
    DeviceDisconnected { device_id: String },
}

pub struct PairingSession {
    pub state: ConnectionState,
    pub peer_info: Option<DeviceInfo>,
    pub store: Arc<PeerStore>,
    pub dbus_tx: tokio::sync::mpsc::Sender<DaemonEvent>,
    pub ws_tx: Arc<Mutex<SplitSink<WebSocketStream<TlsStream<TcpStream>>, Message>>>,
    pub last_pong: Arc<Mutex<Instant>>,
    pub identity: Arc<Identity>,
}

impl PairingSession {
    pub async fn handle_packet(&mut self, packet: Packet) -> Result<()> {
        match packet {
            Packet::Hello { device_id, name, version, fingerprint } => {
                {
                    let hello = Packet::Hello {
                        device_id: self.identity.device_id.clone(),
                        name: self.identity.name.clone(),
                        version: 1u32,
                        fingerprint: self.identity.fingerprint.clone(),
                    };
                    if let Ok(json) = hello.to_json() {
                        let mut sink = self.ws_tx.lock().await;
                        let _ = sink.send(Message::Text(json)).await;
                    }
                }

                let info = DeviceInfo { device_id: device_id.clone(), name: name.clone(), fingerprint: fingerprint.clone(), version };
                self.peer_info = Some(info.clone());
                
                if let Some(stored) = self.store.get_fingerprint(&device_id) {
                    if stored == fingerprint {
                        self.state = ConnectionState::PairedConnected;
                        let _ = self.dbus_tx.send(DaemonEvent::DeviceConnected { device: info.clone() }).await;
                        tracing::info!("Auto-trusted: {}", name);
                    } else {
                        self.state = ConnectionState::Disconnected;
                        let mut sink = self.ws_tx.lock().await;
                        let _ = sink.send(Message::Text(Packet::PairReject { reason: "fingerprint_changed".into() }.to_json()?)).await;
                        let _ = self.dbus_tx.send(DaemonEvent::PairingRejected { device_id: device_id.clone() }).await;
                        return Err(anyhow::anyhow!("Fingerprint mismatch — possible MITM"));
                    }
                } else {
                    self.state = ConnectionState::AwaitingPair;
                    let _ = self.dbus_tx.send(DaemonEvent::PairingRequested { device: info.clone() }).await;
                    tracing::info!("Pairing requested from: {}", name);
                }
            }
            Packet::PairRequest { device_id, name, fingerprint: _ } => {
                tracing::info!("Received PairRequest from {} ({})", name, device_id);
            }
            Packet::PairAccept { fingerprint: _ } => {
                if self.state == ConnectionState::AwaitingPair {
                    if let Some(info) = &self.peer_info {
                        self.store.store_device(info)?;
                        self.state = ConnectionState::PairedConnected;
                        let _ = self.dbus_tx.send(DaemonEvent::PairingCompleted { device_id: info.device_id.clone() }).await;
                        tracing::info!("Pairing accepted from Android side for {}", info.name);
                    }
                }
            }
            Packet::PairReject { reason } => {
                self.state = ConnectionState::Disconnected;
                tracing::warn!("Pairing rejected by Android: {}", reason);
            }
            Packet::Ping { timestamp_ms } => {
                let mut sink = self.ws_tx.lock().await;
                let _ = sink.send(Message::Text(Packet::Pong { timestamp_ms }.to_json()?)).await;
            }
            Packet::Pong { .. } => {
                *self.last_pong.lock().await = Instant::now();
            }
            Packet::Disconnect { reason } => {
                self.state = ConnectionState::Disconnected;
                if let Some(info) = &self.peer_info {
                    let _ = self.dbus_tx.send(DaemonEvent::DeviceDisconnected { device_id: info.device_id.clone() }).await;
                }
                tracing::info!("Graceful disconnect: {}", reason);
            }
        }
        Ok(())
    }

    pub async fn accept_pairing(&mut self) -> Result<()> {
        if let Some(info) = &self.peer_info {
            let mut sink = self.ws_tx.lock().await;
            let _ = sink.send(Message::Text(Packet::PairAccept { fingerprint: info.fingerprint.clone() }.to_json()?)).await;
            self.store.store_device(info)?;
            self.state = ConnectionState::PairedConnected;
            let _ = self.dbus_tx.send(DaemonEvent::PairingCompleted { device_id: info.device_id.clone() }).await;
        }
        Ok(())
    }

    pub async fn reject_pairing(&mut self) -> Result<()> {
        if let Some(info) = &self.peer_info {
            let mut sink = self.ws_tx.lock().await;
            let _ = sink.send(Message::Text(Packet::PairReject { reason: "user_rejected".into() }.to_json()?)).await;
            self.state = ConnectionState::Disconnected;
            let _ = self.dbus_tx.send(DaemonEvent::PairingRejected { device_id: info.device_id.clone() }).await;
        }
        Ok(())
    }
}
