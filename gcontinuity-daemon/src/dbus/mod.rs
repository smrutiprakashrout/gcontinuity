pub mod transport_iface;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use zbus::{Connection, interface};

use crate::pairing::{DaemonEvent, PairingGate};
use crate::store::PeerStore;
use gcontinuity_common::{ConnectionState, DeviceInfo};

pub struct DaemonInterface {
    pub store:            Arc<PeerStore>,
    pub connection_state: Arc<Mutex<ConnectionState>>,
    pub pairing_gate:     PairingGate,
    /// Single source of truth: Some(device) when live connection exists, None otherwise.
    /// Written by PairingSession via set_connected/set_disconnected.
    /// Read by get_connected_device() which the GTK app calls at startup.
    pub connected_device: Arc<Mutex<Option<DeviceInfo>>>,
}

#[interface(name = "org.gcontinuity.Daemon1")]
impl DaemonInterface {
    /// Returns the currently connected device as a JSON string, or an empty
    /// string if no device is connected right now.
    /// The GTK app calls this ONCE at startup to check initial state.
    async fn get_connected_device(&self) -> String {
        let guard = self.connected_device.lock().await;
        match guard.as_ref() {
            Some(device) => serde_json::to_string(device).unwrap_or_default(),
            None         => String::new(),
        }
    }

    async fn accept_pairing(&self, device_id: String) -> zbus::fdo::Result<()> {
        tracing::info!("D-Bus: User accepted pairing for {}", device_id);
        if !self.pairing_gate.resolve(&device_id, true).await {
            return Err(zbus::fdo::Error::Failed(format!(
                "No pending pairing session for device {}", device_id
            )));
        }
        Ok(())
    }

    async fn reject_pairing(&self, device_id: String) -> zbus::fdo::Result<()> {
        tracing::info!("D-Bus: User rejected pairing for {}", device_id);
        if !self.pairing_gate.resolve(&device_id, false).await {
            return Err(zbus::fdo::Error::Failed(format!(
                "No pending pairing session for device {}", device_id
            )));
        }
        Ok(())
    }

    async fn unpair_device(&self, device_id: String) -> zbus::fdo::Result<()> {
        self.store.remove_device(&device_id)
            .map_err(|e| zbus::fdo::Error::Failed(format!("Failed to unpair: {}", e)))?;
        Ok(())
    }

    async fn list_paired_devices(&self) -> zbus::fdo::Result<Vec<String>> {
        let devices = self.store.list_devices()
            .map_err(|e| zbus::fdo::Error::Failed(format!("Failed to list: {}", e)))?;
        Ok(devices.into_iter().filter_map(|d| serde_json::to_string(&d).ok()).collect())
    }

    #[zbus(property)]
    async fn connection_state(&self) -> String {
        let state = self.connection_state.lock().await;
        format!("{:?}", *state)
    }

    #[zbus(signal)]
    async fn pairing_requested(ctxt: &zbus::SignalContext<'_>, device_json: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn pairing_completed(ctxt: &zbus::SignalContext<'_>, device_id: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn pairing_rejected(ctxt: &zbus::SignalContext<'_>, device_id: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn device_connected(ctxt: &zbus::SignalContext<'_>, device_json: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn device_disconnected(ctxt: &zbus::SignalContext<'_>, device_id: &str) -> zbus::Result<()>;
}

pub async fn start_dbus_service(
    store:            Arc<PeerStore>,
    mut event_rx:     broadcast::Receiver<DaemonEvent>,
    conn_state:       Arc<Mutex<ConnectionState>>,
    pairing_gate:     PairingGate,
    connected_device: Arc<Mutex<Option<DeviceInfo>>>,
) -> Result<Connection> {
    let daemon = DaemonInterface {
        store,
        connection_state: conn_state,
        pairing_gate,
        connected_device,
    };

    let connection = Connection::session().await?;
    connection.object_server().at("/org/gcontinuity/Daemon", daemon).await?;
    connection.request_name("org.gcontinuity.Daemon").await?;

    let conn_clone = connection.clone();
    tokio::spawn(async move {
        let interface_ref = conn_clone
            .object_server()
            .interface::<_, DaemonInterface>("/org/gcontinuity/Daemon")
            .await;

        while let Ok(event) = event_rx.recv().await {
            if let Ok(v) = interface_ref.clone() {
                let context = v.signal_context();
                match event {
                    DaemonEvent::PairingRequested { device } => {
                        let _ = DaemonInterface::pairing_requested(
                            context, &serde_json::to_string(&device).unwrap_or_default(),
                        ).await;
                    }
                    DaemonEvent::PairingCompleted { device_id } => {
                        let _ = DaemonInterface::pairing_completed(context, &device_id).await;
                    }
                    DaemonEvent::PairingRejected { device_id } => {
                        let _ = DaemonInterface::pairing_rejected(context, &device_id).await;
                    }
                    DaemonEvent::DeviceConnected { device } => {
                        let _ = DaemonInterface::device_connected(
                            context, &serde_json::to_string(&device).unwrap_or_default(),
                        ).await;
                    }
                    DaemonEvent::DeviceDisconnected { device_id } => {
                        let _ = DaemonInterface::device_disconnected(context, &device_id).await;
                    }
                }
            }
        }
    });

    Ok(connection)
}
