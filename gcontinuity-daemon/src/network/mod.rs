use anyhow::Result;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use futures_util::stream::StreamExt;
use zbus::Connection;
use zbus::proxy;

pub enum NetworkEvent {
    WiFiAvailable,
    WiFiLost,
}

#[proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
    #[zbus(signal)]
    fn state_changed(&self, state: u32) -> zbus::Result<()>;
}

pub struct NetworkWatcher;

impl NetworkWatcher {
    pub async fn run(event_tx: Sender<NetworkEvent>) -> Result<()> {
        let connection = Connection::system().await?;
        let proxy = NetworkManagerProxy::new(&connection).await?;
        
        tracing::info!("Subscribed to NetworkManager signals");
        
        let mut signals = proxy.receive_state_changed().await?;

        while let Some(signal) = signals.next().await {
            let state = signal.args()?.state;
            match state {
                70 => {
                    tracing::info!("NetworkManager: WiFiAvailable (Connected Global)");
                    let _ = event_tx.send(NetworkEvent::WiFiAvailable).await;
                }
                20 => {
                    tracing::info!("NetworkManager: WiFiLost (Disconnected)");
                    let _ = event_tx.send(NetworkEvent::WiFiLost).await;
                }
                _ => {
                    tracing::debug!("NetworkManager: State changed to {}", state);
                }
            }
        }
        
        Ok(())
    }

    pub fn spawn(event_tx: Sender<NetworkEvent>) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = Self::run(event_tx).await {
                tracing::warn!("NetworkManager watcher failed (is it running?): {}", e);
            }
        })
    }
}
