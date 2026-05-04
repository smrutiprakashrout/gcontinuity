//! D-Bus interface com.gcontinuity.Transport
//!
//! ADDED:
//!   - accept_pairing(device_id) — GTK calls this when user clicks Accept
//!   - reject_pairing(device_id) — GTK calls this when user clicks Reject
//!   - list_trusted_devices()    — GTK calls this to populate trusted list on home page
//!   These wire into PairingGate to unblock the waiting handshake task.

use std::sync::Arc;
use tokio::sync::broadcast;
use zbus::{interface, SignalContext};

use crate::store::PeerStore;
use crate::transport::packet::Packet;
use crate::transport::peer::PeerRegistry;
use crate::transport::webrtc::WebRtcManager;
use crate::transport::websocket_server::{PairingGate, TransportEvent};

pub struct TransportInterface {
    pub registry:     Arc<PeerRegistry>,
    pub webrtc:       Arc<WebRtcManager>,
    pub store:        Arc<PeerStore>,
    pub pairing_gate: PairingGate,
}

#[interface(name = "com.gcontinuity.Transport")]
impl TransportInterface {

    async fn send_packet(&self, device_id: &str, json: &str) -> zbus::fdo::Result<()> {
        let packet = Packet::from_json(json)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        self.registry.send_to(device_id, packet).await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn get_connected_devices(&self) -> Vec<(String, String)> {
        self.registry.get_all().await
            .into_iter()
            .map(|h| (h.device_id, h.name))
            .collect()
    }

    async fn get_webrtc_sessions(&self) -> Vec<String> {
        self.webrtc.active_sessions().await
    }

    /// Called by GTK when the user clicks Accept in the pairing dialog.
    async fn accept_pairing(&self, device_id: &str) -> zbus::fdo::Result<()> {
        let resolved = self.pairing_gate.resolve(device_id, true).await;
        if resolved {
            tracing::info!("GTK accepted pairing for {device_id}");
            Ok(())
        } else {
            tracing::warn!("accept_pairing: no pending pairing for {device_id}");
            Err(zbus::fdo::Error::Failed(
                format!("No pending pairing for {device_id}")
            ))
        }
    }

    /// Called by GTK when the user clicks Reject in the pairing dialog.
    async fn reject_pairing(&self, device_id: &str) -> zbus::fdo::Result<()> {
        let resolved = self.pairing_gate.resolve(device_id, false).await;
        if resolved {
            tracing::info!("GTK rejected pairing for {device_id}");
            Ok(())
        } else {
            tracing::warn!("reject_pairing: no pending pairing for {device_id}");
            Err(zbus::fdo::Error::Failed(
                format!("No pending pairing for {device_id}")
            ))
        }
    }

    /// Returns list of (device_id, name) for all trusted devices in PeerStore.
    /// GTK uses this to populate the Trusted Devices section on the home page.
    async fn list_trusted_devices(&self) -> Vec<(String, String)> {
        self.store.list_devices().unwrap_or_default()
            .into_iter()
            .map(|d| (d.device_id, d.name))
            .collect()
    }

    // ── Signals ───────────────────────────────────────────────────────────────

    #[zbus(signal)]
    pub async fn device_connected(
        ctx: &SignalContext<'_>, device_id: &str, name: &str, addr: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn device_disconnected(
        ctx: &SignalContext<'_>, device_id: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn pairing_requested(
        ctx: &SignalContext<'_>, device_id: &str, name: &str, fingerprint: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn pairing_accepted(
        ctx: &SignalContext<'_>, device_id: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn pairing_rejected(
        ctx: &SignalContext<'_>, device_id: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn packet_received(
        ctx: &SignalContext<'_>, device_id: &str, json: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn file_progress(
        ctx: &SignalContext<'_>, file_id: &str, bytes_done: u64, total: u64,
    ) -> zbus::Result<()>;
}

pub async fn start_transport_dbus_service(
    registry:     Arc<PeerRegistry>,
    webrtc:       Arc<WebRtcManager>,
    store:        Arc<PeerStore>,
    pairing_gate: PairingGate,
    mut event_rx: broadcast::Receiver<TransportEvent>,
) -> anyhow::Result<()> {
    let connection = zbus::Connection::session().await?;
    connection.request_name("com.gcontinuity.Daemon").await?;

    let iface = TransportInterface { registry, webrtc, store, pairing_gate };
    connection.object_server()
        .at("/com/gcontinuity/Transport", iface)
        .await?;

    tracing::info!("D-Bus Transport interface registered (com.gcontinuity.Daemon)");

    loop {
        let iface_ref = connection.object_server()
            .interface::<_, TransportInterface>("/com/gcontinuity/Transport")
            .await?;
        let ctx = iface_ref.signal_context();

        match event_rx.recv().await {
            Ok(TransportEvent::DeviceConnected { device_id, name, addr }) => {
                TransportInterface::device_connected(ctx, &device_id, &name, &addr).await.ok();
            }
            Ok(TransportEvent::DeviceDisconnected { device_id }) => {
                TransportInterface::device_disconnected(ctx, &device_id).await.ok();
            }
            Ok(TransportEvent::PairingRequested { device_id, name, fingerprint }) => {
                TransportInterface::pairing_requested(ctx, &device_id, &name, &fingerprint).await.ok();
            }
            Ok(TransportEvent::PairingAccepted { device_id }) => {
                TransportInterface::pairing_accepted(ctx, &device_id).await.ok();
            }
            Ok(TransportEvent::PairingRejected { device_id }) => {
                TransportInterface::pairing_rejected(ctx, &device_id).await.ok();
            }
            Ok(TransportEvent::PacketReceived { device_id, packet }) => {
                TransportInterface::packet_received(ctx, &device_id, &packet.to_json()).await.ok();
            }
            Ok(TransportEvent::FileProgress { file_id, bytes_done, total }) => {
                TransportInterface::file_progress(ctx, &file_id, bytes_done, total).await.ok();
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("D-Bus bridge lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::info!("Transport event channel closed — stopping");
                break;
            }
        }
    }
    Ok(())
}
