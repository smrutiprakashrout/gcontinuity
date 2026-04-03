use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_rustls::rustls;
use tracing_subscriber::EnvFilter;

mod identity;
mod config;
mod dbus;
mod keepalive;
mod mdns;
mod network;
mod pairing;
mod server;
mod store;

use gcontinuity_common::ConnectionState;
use identity::Identity;
use store::PeerStore;
use pairing::DaemonEvent;
use mdns::MdnsService;
use server::WsServer;
use network::{NetworkWatcher, NetworkEvent};

#[tokio::main]
async fn main() -> Result<()> {
    // Install the ring crypto provider for rustls before any TLS setup.
    // Required in rustls 0.23+ — must be called before ServerConfig::builder().
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install ring CryptoProvider");

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let device_name = "Smruti's Linux".to_string(); // In a real app, read from config.toml/hostname
    let identity = Arc::new(Identity::load_or_create(&device_name).await?);
    
    let store = Arc::new(PeerStore::open()?);
    let (event_tx, _) = tokio::sync::broadcast::channel::<DaemonEvent>(32);
    let conn_state = Arc::new(Mutex::new(ConnectionState::Idle));

    let _dbus_conn = dbus::start_dbus_service(store.clone(), event_tx.subscribe(), conn_state.clone()).await?;

    let (nm_tx, mut nm_rx) = tokio::sync::mpsc::channel::<NetworkEvent>(8);
    NetworkWatcher::spawn(nm_tx);

    let mdns = MdnsService::new(device_name.clone(), identity.device_id.clone());
    mdns.run_in_background().await;

    let server = WsServer::new(identity.clone(), store.clone(), event_tx.clone());

    tracing::info!("GContinuity daemon started");
    tracing::info!("  Device: {}", device_name);
    tracing::info!("  ID:     {}", identity.device_id);
    tracing::info!("  FP:     {}", identity.fingerprint);
    tracing::info!("  Listen: wss://0.0.0.0:52000");

    tokio::select! {
        res = server.run() => {
            if let Err(e) = res {
                tracing::error!("Server error: {}", e);
            }
        }
        _ = tokio::spawn(async move {
            while let Some(event) = nm_rx.recv().await {
                if let NetworkEvent::WiFiAvailable = event {
                    tracing::info!("WiFi Available — would trigger mDNS re-browse");
                }
            }
        }) => {}
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Shutting down gracefully...");
        }
    }

    Ok(())
}
