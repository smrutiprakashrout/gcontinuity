//! GContinuity daemon entry point — Phase 2.
//!
//! CHANGES:
//!   - PairingGate created here and passed to both run_server and transport D-Bus.
//!   - PeerStore passed to run_server (for trust checks) and transport D-Bus.
//!   - run_server signature updated: added store + pairing_gate args.
//!   - start_transport_dbus_service signature updated: added store + pairing_gate.

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use tokio_rustls::rustls;

mod config;
mod dbus;
mod identity;
mod keepalive;
mod mdns;
mod network;
mod pairing;
mod server;
mod store;
mod tls;
mod transport;

use config::load_config;
use gcontinuity_common::{ConnectionState, DeviceInfo};
use identity::Identity;
use store::PeerStore;
use pairing::{DaemonEvent, PairingGate as Phase1Gate};
use mdns::MdnsService;
use network::{NetworkWatcher, NetworkEvent};
use transport::{PeerRegistry, WebRtcManager, FeatureEvent};
use transport::websocket_server::{TransportEvent, PairingGate};

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install ring CryptoProvider");

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("gcontinuity_daemon=debug".parse()?)
                .add_directive("gcontinuity=info".parse()?),
        )
        .init();

    let config = Arc::new(load_config()?);
    tracing::info!("Config loaded, port={}", config.port);

    let tls = Arc::new(tls::load_or_generate(&config.data_dir).await?);
    let tls_fingerprint = hex::encode(tls.cert_sha256);
    tracing::info!(fingerprint = %tls_fingerprint, "TLS ready");

    // Phase 1 services
    let device_name = config.device_name.clone();
    let identity    = Arc::new(Identity::load_or_create(&device_name).await?);
    let store       = Arc::new(PeerStore::open()?);

    let (p1_event_tx, _) = broadcast::channel::<DaemonEvent>(32);
    let conn_state        = Arc::new(Mutex::new(ConnectionState::Idle));
    let p1_gate           = Phase1Gate::new();
    let connected_device: Arc<Mutex<Option<DeviceInfo>>> = Arc::new(Mutex::new(None));

    let _dbus_conn = dbus::start_dbus_service(
        store.clone(),
        p1_event_tx.subscribe(),
        conn_state,
        p1_gate,
        connected_device,
    ).await?;

    let mdns = MdnsService::new(device_name.clone(), identity.device_id.clone());
    mdns.run_in_background().await;

    let (nm_tx, mut nm_rx) = tokio::sync::mpsc::channel::<NetworkEvent>(8);
    NetworkWatcher::spawn(nm_tx);
    tokio::spawn(async move {
        while let Some(event) = nm_rx.recv().await {
            if let NetworkEvent::WiFiAvailable = event {
                tracing::info!("WiFi available — mDNS re-browse would trigger here");
            }
        }
    });

    // Phase 2 services
    let registry     = Arc::new(PeerRegistry::new());
    let webrtc       = Arc::new(WebRtcManager::new());
    let pairing_gate = PairingGate::new();
    let (event_tx, event_rx) = broadcast::channel::<TransportEvent>(256);
    let (feature_tx, _feature_rx) = mpsc::channel::<FeatureEvent>(256);

    let cancel    = CancellationToken::new();
    let cancel_ws = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Shutdown signal received");
        cancel_ws.cancel();
    });

    tracing::info!(
        device    = %device_name,
        device_id = %identity.device_id,
        fp_p1     = %identity.fingerprint,
        port      = config.port,
        "GContinuity daemon started"
    );

    tokio::try_join!(
        transport::run_server(
            config.clone(),
            tls.clone(),
            store.clone(),
            pairing_gate.clone(),
            identity.device_id.clone(),
            tls_fingerprint,
            registry.clone(),
            event_tx,
            feature_tx,
            cancel,
        ),
        dbus::transport_iface::start_transport_dbus_service(
            registry,
            webrtc,
            store,
            pairing_gate,
            event_rx,
        ),
    )?;

    tracing::info!("gcontinuity-daemon stopped cleanly");
    Ok(())
}
