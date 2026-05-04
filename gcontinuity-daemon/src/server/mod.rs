// gcontinuity-daemon/src/server/mod.rs
//
// CHANGES FROM OLD VERSION:
//   None structurally — this file was already correct.
//   Port 52000 is correct and matches Android's MdnsDiscovery default.
//   tokio_tungstenite::accept_async upgrades any path — no path routing needed
//   since OkHttp connects to wss://host:port (no path suffix in WsClient).
//
// NOTE: Android's TransportManager.kt uses wss://$host:$port/gcontinuity
// but WsClient.kt (the Phase 1 client) uses wss://$host:$port (no path).
// The daemon uses tokio_tungstenite::accept_async which accepts any path.
// No change needed here — both paths are accepted transparently.

#![allow(dead_code)] // Phase 1 — reactivated in Phase 3
use anyhow::Result;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;

use crate::identity::Identity;
use crate::store::PeerStore;
use crate::pairing::{DaemonEvent, PairingGate};
use gcontinuity_common::DeviceInfo;

pub mod connection;

/// The port the daemon listens on.
/// Must match `MdnsDiscovery.port` default (52000) and the port advertised
/// via mDNS so Android can discover it automatically.
pub const DAEMON_PORT: u16 = 52000;

pub struct WsServer {
    pub identity:         Arc<Identity>,
    pub store:            Arc<PeerStore>,
    pub dbus_tx:          tokio::sync::broadcast::Sender<DaemonEvent>,
    pub gate:             PairingGate,
    pub connected_device: Arc<Mutex<Option<DeviceInfo>>>,
}

impl WsServer {
    pub fn new(
        identity:         Arc<Identity>,
        store:            Arc<PeerStore>,
        dbus_tx:          tokio::sync::broadcast::Sender<DaemonEvent>,
        gate:             PairingGate,
        connected_device: Arc<Mutex<Option<DeviceInfo>>>,
    ) -> Self {
        Self { identity, store, dbus_tx, gate, connected_device }
    }

    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind(format!("0.0.0.0:{DAEMON_PORT}")).await?;

        // Parse the identity's PEM strings into rustls types.
        let mut certs_reader = std::io::BufReader::new(self.identity.cert_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut certs_reader)
            .collect::<Result<Vec<_>, _>>()?;

        let mut key_reader = std::io::BufReader::new(self.identity.key_pem.as_bytes());
        let keys = rustls_pemfile::private_key(&mut key_reader)?
            .expect("valid private key must exist in identity");

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, keys)?;

        let tls_acceptor = TlsAcceptor::from(Arc::new(config));
        tracing::info!(
            "Listening for WebSocket connections on wss://0.0.0.0:{}",
            DAEMON_PORT
        );

        loop {
            let (tcp_stream, peer_addr) = listener.accept().await?;
            tracing::info!("New TCP connection from {}", peer_addr);

            let tls_acceptor_clone     = tls_acceptor.clone();
            let store_clone            = self.store.clone();
            let dbus_tx_clone          = self.dbus_tx.clone();
            let identity_clone         = self.identity.clone();
            let gate_clone             = self.gate.clone();
            let connected_device_clone = self.connected_device.clone();

            tokio::spawn(async move {
                match tls_acceptor_clone.accept(tcp_stream).await {
                    Ok(tls_stream) => {
                        // accept_async accepts any HTTP Upgrade path — both
                        // wss://host:port and wss://host:port/gcontinuity work.
                        match tokio_tungstenite::accept_async(tls_stream).await {
                            Ok(ws_stream) => {
                                connection::handle(
                                    ws_stream,
                                    peer_addr,
                                    store_clone,
                                    dbus_tx_clone,
                                    identity_clone,
                                    gate_clone,
                                    connected_device_clone,
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::error!(
                                    "WebSocket upgrade failed for {}: {}",
                                    peer_addr,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("TLS handshake failed for {}: {}", peer_addr, e);
                    }
                }
            });
        }
    }
}
