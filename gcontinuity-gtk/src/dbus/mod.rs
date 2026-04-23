use anyhow::Result;
use futures_util::StreamExt;
use zbus::Connection;
use zbus::proxy;

#[proxy(
    interface = "org.gcontinuity.Daemon1",
    default_service = "org.gcontinuity.Daemon",
    default_path = "/org/gcontinuity/Daemon"
)]
trait Daemon {
    async fn accept_pairing(&self, device_id: &str) -> zbus::Result<()>;
    async fn reject_pairing(&self, device_id: &str) -> zbus::Result<()>;
    async fn unpair_device(&self, device_id: &str) -> zbus::Result<()>;
    async fn list_paired_devices(&self) -> zbus::Result<Vec<String>>;

    #[zbus(property)]
    fn connection_state(&self) -> zbus::Result<String>;

    #[zbus(signal)]
    async fn pairing_requested(&self, device_json: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn pairing_completed(&self, device_id: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn pairing_rejected(&self, device_id: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn device_connected(&self, device_json: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn device_disconnected(&self, device_id: String) -> zbus::Result<()>;
}

enum DaemonSignal {
    Connected(String),
    Disconnected(String),
}

/// Spawn a background thread that listens for daemon D-Bus signals and
/// delivers them to the glib main loop via `async_channel`.
/// Both callbacks run on the **glib main thread** — GTK objects are safe.
pub fn start_dbus_listener(
    on_connected: impl Fn(String) + 'static,
    on_disconnected: impl Fn(String) + 'static,
) {
    let (tx, rx) = async_channel::unbounded::<DaemonSignal>();

    // Background thread owns the tokio runtime and the D-Bus connection.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime for D-Bus listener");
        rt.block_on(async move {
            if let Err(e) = run_listener(tx).await {
                tracing::warn!("D-Bus listener stopped: {}", e);
            }
        });
    });

    // glib::spawn_future_local runs on the glib main loop — no Send needed.
    glib::spawn_future_local(async move {
        while let Ok(signal) = rx.recv().await {
            match signal {
                DaemonSignal::Connected(json) => on_connected(json),
                DaemonSignal::Disconnected(id) => on_disconnected(id),
            }
        }
    });
}

async fn run_listener(tx: async_channel::Sender<DaemonSignal>) -> Result<()> {
    let connection = Connection::session().await?;
    let proxy = DaemonProxy::new(&connection).await?;

    let mut conn_stream = proxy.receive_device_connected().await?;
    let mut disc_stream = proxy.receive_device_disconnected().await?;

    loop {
        tokio::select! {
            Some(signal) = conn_stream.next() => {
                if let Ok(args) = signal.args() {
                    let _ = tx.send(DaemonSignal::Connected(args.device_json.to_owned())).await;
                }
            }
            Some(signal) = disc_stream.next() => {
                if let Ok(args) = signal.args() {
                    let _ = tx.send(DaemonSignal::Disconnected(args.device_id.to_owned())).await;
                }
            }
            else => break,
        }
    }

    Ok(())
}
