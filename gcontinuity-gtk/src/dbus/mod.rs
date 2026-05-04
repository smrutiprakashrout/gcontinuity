//! D-Bus client for the GTK settings app.
//! Talks to com.gcontinuity.Transport / /com/gcontinuity/Transport (Phase 2).
//!
//! ADDED:
//!   - accept_pairing() / reject_pairing() — real D-Bus method calls that
//!     unblock the daemon's handshake task via PairingGate.
//!   - list_trusted_devices() — populates the Trusted Devices home page section.
//!   Permanent reconnect loop — never dies after first disconnect.

use anyhow::Result;
use futures_util::StreamExt;
use zbus::Connection;
use zbus::proxy;

#[proxy(
    interface = "com.gcontinuity.Transport",
    default_service = "com.gcontinuity.Daemon",
    default_path = "/com/gcontinuity/Transport"
)]
trait Transport {
    async fn send_packet(&self, device_id: &str, json: &str) -> zbus::Result<()>;
    async fn get_connected_devices(&self) -> zbus::Result<Vec<(String, String)>>;
    async fn get_webrtc_sessions(&self) -> zbus::Result<Vec<String>>;
    async fn accept_pairing(&self, device_id: &str) -> zbus::Result<()>;
    async fn reject_pairing(&self, device_id: &str) -> zbus::Result<()>;
    async fn list_trusted_devices(&self) -> zbus::Result<Vec<(String, String)>>;

    #[zbus(signal)]
    async fn device_connected(
        &self, device_id: String, name: String, addr: String,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn device_disconnected(&self, device_id: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn pairing_requested(
        &self, device_id: String, name: String, fingerprint: String,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn pairing_accepted(&self, device_id: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn pairing_rejected(&self, device_id: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn packet_received(&self, device_id: String, json: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn file_progress(
        &self, file_id: String, bytes_done: u64, total: u64,
    ) -> zbus::Result<()>;
}

// ── Initial state ─────────────────────────────────────────────────────────────

pub enum InitialState {
    Connected(String, String),  // (device_id, name)
    Waiting,
    DaemonNotRunning,
}

pub fn query_initial_state(on_result: impl Fn(InitialState) + 'static) {
    let (tx, rx) = async_channel::bounded::<InitialState>(1);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().expect("tokio rt");
        rt.block_on(async move { let _ = tx.send(do_query().await).await; });
    });
    gtk4::glib::spawn_future_local(async move {
        if let Ok(state) = rx.recv().await { on_result(state); }
    });
}

async fn do_query() -> InitialState {
    for attempt in 0..8 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let conn  = match Connection::session().await { Ok(c) => c, Err(_) => continue };
        let proxy = match TransportProxy::new(&conn).await { Ok(p) => p, Err(_) => continue };
        match proxy.get_connected_devices().await {
            Ok(devices) if !devices.is_empty() => {
                let (id, name) = devices.into_iter().next().unwrap();
                return InitialState::Connected(id, name);
            }
            Ok(_)  => return InitialState::Waiting,
            Err(_) => continue,
        }
    }
    InitialState::DaemonNotRunning
}

// ── Trusted devices query ─────────────────────────────────────────────────────

/// Fetch trusted devices from daemon for home page population.
pub fn query_trusted_devices(on_result: impl Fn(Vec<(String, String)>) + 'static) {
    let (tx, rx) = async_channel::bounded::<Vec<(String, String)>>(1);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().expect("tokio rt");
        rt.block_on(async move {
            let conn  = Connection::session().await.ok();
            let proxy = if let Some(c) = conn {
                TransportProxy::new(&c).await.ok()
            } else { None };
            let devices = if let Some(p) = proxy {
                p.list_trusted_devices().await.unwrap_or_default()
            } else { vec![] };
            let _ = tx.send(devices).await;
        });
    });
    gtk4::glib::spawn_future_local(async move {
        if let Ok(devices) = rx.recv().await { on_result(devices); }
    });
}

// ── Signal types ──────────────────────────────────────────────────────────────

pub enum DaemonSignal {
    PairingRequested { device_id: String, name: String, fingerprint: String },
    Connected        { device_id: String, name: String, addr: String },
    Disconnected     { device_id: String },
    PairingAccepted  { device_id: String },
    PairingRejected  { device_id: String },
}

// ── Live listener ─────────────────────────────────────────────────────────────

pub fn start_dbus_listener(
    on_pairing_requested: impl Fn(String, String, String) + 'static,
    on_connected:         impl Fn(String, String, String) + 'static,
    on_disconnected:      impl Fn(String) + 'static,
    on_pairing_accepted:  impl Fn(String) + 'static,
    on_pairing_rejected:  impl Fn(String) + 'static,
) {
    let (tx, rx) = async_channel::unbounded::<DaemonSignal>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().expect("tokio rt");
        rt.block_on(async move {
            loop {
                match run_listener(tx.clone()).await {
                    Ok(()) => {
                        tracing::info!("D-Bus listener stream ended — reconnecting in 1s");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    Err(e) => {
                        tracing::warn!("D-Bus listener error: {e} — reconnecting in 2s");
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        });
    });

    gtk4::glib::spawn_future_local(async move {
        while let Ok(signal) = rx.recv().await {
            match signal {
                DaemonSignal::PairingRequested { device_id, name, fingerprint } =>
                    on_pairing_requested(device_id, name, fingerprint),
                DaemonSignal::Connected { device_id, name, addr } =>
                    on_connected(device_id, name, addr),
                DaemonSignal::Disconnected { device_id } =>
                    on_disconnected(device_id),
                DaemonSignal::PairingAccepted { device_id } =>
                    on_pairing_accepted(device_id),
                DaemonSignal::PairingRejected { device_id } =>
                    on_pairing_rejected(device_id),
            }
        }
    });
}

async fn run_listener(tx: async_channel::Sender<DaemonSignal>) -> Result<()> {
    let connection = Connection::session().await?;
    let proxy      = TransportProxy::new(&connection).await?;

    let mut pair_req_stream    = proxy.receive_pairing_requested().await?;
    let mut conn_stream        = proxy.receive_device_connected().await?;
    let mut disc_stream        = proxy.receive_device_disconnected().await?;
    let mut pair_accept_stream = proxy.receive_pairing_accepted().await?;
    let mut pair_reject_stream = proxy.receive_pairing_rejected().await?;

    tracing::info!("D-Bus listener connected to com.gcontinuity.Transport");

    loop {
        tokio::select! {
            msg = pair_req_stream.next() => match msg {
                Some(s) => if let Ok(a) = s.args() {
                    let _ = tx.send(DaemonSignal::PairingRequested {
                        device_id: a.device_id.to_owned(),
                        name: a.name.to_owned(),
                        fingerprint: a.fingerprint.to_owned(),
                    }).await;
                },
                None => { tracing::info!("pairing_requested stream ended"); return Ok(()); }
            },
            msg = conn_stream.next() => match msg {
                Some(s) => if let Ok(a) = s.args() {
                    let _ = tx.send(DaemonSignal::Connected {
                        device_id: a.device_id.to_owned(),
                        name: a.name.to_owned(),
                        addr: a.addr.to_owned(),
                    }).await;
                },
                None => { tracing::info!("device_connected stream ended"); return Ok(()); }
            },
            msg = disc_stream.next() => match msg {
                Some(s) => if let Ok(a) = s.args() {
                    let _ = tx.send(DaemonSignal::Disconnected {
                        device_id: a.device_id.to_owned(),
                    }).await;
                },
                None => { tracing::info!("device_disconnected stream ended"); return Ok(()); }
            },
            msg = pair_accept_stream.next() => match msg {
                Some(s) => if let Ok(a) = s.args() {
                    let _ = tx.send(DaemonSignal::PairingAccepted {
                        device_id: a.device_id.to_owned(),
                    }).await;
                },
                None => { tracing::info!("pairing_accepted stream ended"); return Ok(()); }
            },
            msg = pair_reject_stream.next() => match msg {
                Some(s) => if let Ok(a) = s.args() {
                    let _ = tx.send(DaemonSignal::PairingRejected {
                        device_id: a.device_id.to_owned(),
                    }).await;
                },
                None => { tracing::info!("pairing_rejected stream ended"); return Ok(()); }
            },
        }
    }
}

// ── One-shot D-Bus method calls ───────────────────────────────────────────────

/// Called when GTK user clicks Accept in the pairing dialog.
/// Delivers the decision to the daemon's PairingGate to unblock the handshake.
pub fn dbus_accept_pairing(device_id: String) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().unwrap();
        rt.block_on(async move {
            if let Ok(conn) = Connection::session().await {
                if let Ok(proxy) = TransportProxy::new(&conn).await {
                    match proxy.accept_pairing(&device_id).await {
                        Ok(()) => tracing::info!("accept_pairing sent for {device_id}"),
                        Err(e) => tracing::error!("accept_pairing failed: {e}"),
                    }
                }
            }
        });
    });
}

/// Called when GTK user clicks Reject in the pairing dialog.
pub fn dbus_reject_pairing(device_id: String) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().unwrap();
        rt.block_on(async move {
            if let Ok(conn) = Connection::session().await {
                if let Ok(proxy) = TransportProxy::new(&conn).await {
                    match proxy.reject_pairing(&device_id).await {
                        Ok(()) => tracing::info!("reject_pairing sent for {device_id}"),
                        Err(e) => tracing::error!("reject_pairing failed: {e}"),
                    }
                }
            }
        });
    });
}
