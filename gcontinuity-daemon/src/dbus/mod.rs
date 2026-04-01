use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use zbus::{Connection, interface};

use crate::pairing::DaemonEvent;
use crate::store::PeerStore;
use gcontinuity_common::ConnectionState;

pub struct DaemonInterface {
    pub store: Arc<PeerStore>,
    pub connection_state: Arc<Mutex<ConnectionState>>,
    // We hold a receiver but use it in a spawned task to emit signals
}

#[interface(name = "org.gcontinuity.Daemon1")]
impl DaemonInterface {
    async fn accept_pairing(&self, device_id: String) -> zbus::fdo::Result<()> {
        // Find a way to send back to PairingSession
        tracing::info!("D-Bus: User accepted pairing for {}", device_id);
        Ok(())
    }

    async fn reject_pairing(&self, device_id: String) -> zbus::fdo::Result<()> {
        tracing::info!("D-Bus: User rejected pairing for {}", device_id);
        Ok(())
    }

    async fn unpair_device(&self, device_id: String) -> zbus::fdo::Result<()> {
        self.store.remove_device(&device_id).map_err(|e| zbus::fdo::Error::Failed(format!("Failed to unpair: {}", e)))?;
        Ok(())
    }

    async fn list_paired_devices(&self) -> zbus::fdo::Result<Vec<String>> {
        let devices = self.store.list_devices().map_err(|e| zbus::fdo::Error::Failed(format!("Failed to list: {}", e)))?;
        let json_list = devices.into_iter()
            .filter_map(|d| serde_json::to_string(&d).ok())
            .collect();
        Ok(json_list)
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
    store: Arc<PeerStore>,
    mut event_rx: broadcast::Receiver<DaemonEvent>,
    conn_state: Arc<Mutex<ConnectionState>>
) -> Result<Connection> {
    let daemon = DaemonInterface {
        store,
        connection_state: conn_state,
    };

    let connection = Connection::session().await?;
    connection.object_server().at("/org/gcontinuity/Daemon", daemon).await?;
    connection.request_name("org.gcontinuity.Daemon").await?;

    // Spawn task to emit D-Bus signals from internal DaemonEvents
    let conn_clone = connection.clone();
    tokio::spawn(async move {
        // Need to wait for object_server to be ready. 
        // We use proxy to emit signals.
        // Actually, zbus 4 requires using the Server to emit signals. We have to do it through the struct or object server.
        // For simplicity in zbus 4:
        let interface_ref = conn_clone.object_server().interface::<_, DaemonInterface>("/org/gcontinuity/Daemon").await;
        
        while let Ok(event) = event_rx.recv().await {
            if let Ok(v) = interface_ref.clone() {
                let context = v.signal_context();
                match event {
                    DaemonEvent::PairingRequested { device } => {
                        let _ = DaemonInterface::pairing_requested(context, &serde_json::to_string(&device).unwrap_or_default()).await;
                    }
                    DaemonEvent::PairingCompleted { device_id } => {
                        let _ = DaemonInterface::pairing_completed(context, &device_id).await;
                    }
                    DaemonEvent::PairingRejected { device_id } => {
                        let _ = DaemonInterface::pairing_rejected(context, &device_id).await;
                    }
                    DaemonEvent::DeviceConnected { device } => {
                        let _ = DaemonInterface::device_connected(context, &serde_json::to_string(&device).unwrap_or_default()).await;
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
