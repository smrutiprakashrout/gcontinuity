use anyhow::Result;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;

use crate::identity::Identity;
use crate::store::PeerStore;
use crate::pairing::DaemonEvent;

pub mod connection;

pub struct WsServer {
    pub identity: Arc<Identity>,
    pub store: Arc<PeerStore>,
    pub dbus_tx: tokio::sync::broadcast::Sender<DaemonEvent>,
}

impl WsServer {
    pub fn new(identity: Arc<Identity>, store: Arc<PeerStore>, dbus_tx: tokio::sync::broadcast::Sender<DaemonEvent>) -> Self {
        Self { identity, store, dbus_tx }
    }

    pub async fn run(&self) -> Result<()> {
        let listener = TcpListener::bind("0.0.0.0:52000").await?;
        
        let mut certs_reader = std::io::BufReader::new(self.identity.cert_pem.as_bytes());
        let certs = rustls_pemfile::certs(&mut certs_reader)
            .collect::<Result<Vec<_>, _>>()?;
            
        let mut key_reader = std::io::BufReader::new(self.identity.key_pem.as_bytes());
        let keys = rustls_pemfile::private_key(&mut key_reader)?
            .expect("Valid private key not found in identity_key");
            
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, keys)?;
            
        let tls_acceptor = TlsAcceptor::from(Arc::new(config));

        tracing::info!("Listening for WebSocket connections on wss://0.0.0.0:52000");

        loop {
            let (tcp_stream, peer_addr) = listener.accept().await?;
            tracing::info!("New connection from {}", peer_addr);
            
            let tls_acceptor_clone = tls_acceptor.clone();
            let store_clone = self.store.clone();
            let dbus_tx_clone = self.dbus_tx.clone();
            
            tokio::spawn(async move {
                match tls_acceptor_clone.accept(tcp_stream).await {
                    Ok(tls_stream) => {
                        match tokio_tungstenite::accept_async(tls_stream).await {
                            Ok(ws_stream) => {
                                connection::handle(ws_stream, peer_addr, store_clone, dbus_tx_clone).await;
                            }
                            Err(e) => tracing::error!("WebSocket upgrade failed: {}", e),
                        }
                    }
                    Err(e) => tracing::error!("TLS connection failed: {}", e),
                }
            });
        }
    }
}
