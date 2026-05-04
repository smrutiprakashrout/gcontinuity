//! Connection status page (Phase 2).
//!
//! Subscribes to `com.gcontinuity.Transport` D-Bus signals and displays:
//!   • Device name, IP address, session uptime (live 1-second counter)
//!   • Status badge: green Connected / amber Reconnecting / red Disconnected
//!   • "Disconnect" button
//!   • Collapsible debug panel: last 50 packets as a scrollable list

use adw::prelude::*;
use gtk4::glib;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use zbus::Connection;
use zbus::proxy;

// ── D-Bus proxy for the Phase 2 Transport interface ───────────────────────────

#[proxy(
    interface = "com.gcontinuity.Transport",
    default_service = "com.gcontinuity.Daemon",
    default_path = "/com/gcontinuity/Transport"
)]
trait Transport {
    async fn send_packet(&self, device_id: &str, json: &str) -> zbus::Result<()>;
    async fn get_connected_devices(&self) -> zbus::Result<Vec<(String, String)>>;

    #[zbus(signal)]
    async fn device_connected(&self, device_id: String, name: String, addr: String)
        -> zbus::Result<()>;
    #[zbus(signal)]
    async fn device_disconnected(&self, device_id: String) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn packet_received(&self, device_id: String, json: String) -> zbus::Result<()>;
}

// ── Internal state ────────────────────────────────────────────────────────────

struct PageState {
    device_id:    Option<String>,
    device_name:  Option<String>,
    ip_addr:      Option<String>,
    connected_at: Option<Instant>,
    packets:      Vec<String>,   // last 50 packets (JSON strings)
}

impl PageState {
    fn new() -> Self {
        Self {
            device_id:    None,
            device_name:  None,
            ip_addr:      None,
            connected_at: None,
            packets:      Vec::new(),
        }
    }

    fn push_packet(&mut self, json: String) {
        if self.packets.len() >= 50 {
            self.packets.remove(0);
        }
        self.packets.push(json);
    }
}

// ── Public builder ────────────────────────────────────────────────────────────

/// Build the connection status page widget.
///
/// Returns the page root widget; call `.present()` or add it to a stack.
/// Signal subscriptions are set up internally via `glib::spawn_future_local`.
pub fn build_connection_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .name("connection")
        .title("Connection")
        .icon_name("network-wireless-symbolic")
        .build();

    // ── Status group ──────────────────────────────────────────────────────────
    let status_group = adw::PreferencesGroup::builder()
        .title("Device Status")
        .build();

    let name_row = adw::ActionRow::builder()
        .title("Device")
        .subtitle("Not connected")
        .build();

    let status_badge = gtk4::Label::builder()
        .label("Disconnected")
        .css_classes(["error"])
        .build();
    name_row.add_suffix(&status_badge);

    let ip_row = adw::ActionRow::builder()
        .title("IP Address")
        .subtitle("—")
        .build();

    let uptime_row = adw::ActionRow::builder()
        .title("Session Uptime")
        .subtitle("—")
        .build();

    let disconnect_btn = gtk4::Button::builder()
        .label("Disconnect")
        .css_classes(["destructive-action", "flat"])
        .sensitive(false)
        .build();
    uptime_row.add_suffix(&disconnect_btn);

    status_group.add(&name_row);
    status_group.add(&ip_row);
    status_group.add(&uptime_row);
    page.add(&status_group);

    // ── Debug packet log (collapsible) ────────────────────────────────────────
    let debug_group = adw::PreferencesGroup::builder()
        .title("Debug")
        .build();

    let packet_store = gtk4::StringList::new(&[]);
    let list_view = {
        let factory = gtk4::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
            item.set_child(Some(
                &gtk4::Label::builder()
                    .xalign(0.0)
                    .css_classes(["monospace", "caption"])
                    .wrap(true)
                    .max_width_chars(60)
                    .build(),
            ));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
            if let Some(obj) = item.item() {
                let string_obj = obj.downcast_ref::<gtk4::StringObject>().unwrap();
                if let Some(lbl) = item.child().and_downcast::<gtk4::Label>() {
                    lbl.set_label(&string_obj.string());
                }
            }
        });
        let selection = gtk4::NoSelection::new(Some(packet_store.clone()));
        gtk4::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .build()
    };
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .min_content_height(120)
        .max_content_height(240)
        .child(&list_view)
        .build();
    let expander = adw::ExpanderRow::builder()
        .title("Packet Log")
        .subtitle("Last 50 packets")
        .build();
    expander.add_row(
        &adw::PreferencesRow::builder()
            .child(&scroll)
            .build(),
    );
    debug_group.add(&expander);
    page.add(&debug_group);

    // ── Shared state ──────────────────────────────────────────────────────────
    let state = Rc::new(RefCell::new(PageState::new()));

    // ── Uptime ticker (every 1 second) ────────────────────────────────────────
    let uptime_row_tick = uptime_row.clone();
    let state_tick = state.clone();
    glib::timeout_add_seconds_local(1, move || {
        let s = state_tick.borrow();
        if let Some(start) = s.connected_at {
            let secs = start.elapsed().as_secs();
            let formatted = format!("{}h {:02}m {:02}s", secs / 3600, (secs % 3600) / 60, secs % 60);
            uptime_row_tick.set_subtitle(&formatted);
        }
        glib::ControlFlow::Continue
    });

    // ── Disconnect button ─────────────────────────────────────────────────────
    let state_disc = state.clone();
    let status_badge_disc = status_badge.clone();
    let name_row_disc = name_row.clone();
    let ip_row_disc = ip_row.clone();
    let uptime_row_disc = uptime_row.clone();
    let disconnect_btn_disc = disconnect_btn.clone();
    disconnect_btn.connect_clicked(move |_| {
        let device_id = state_disc.borrow().device_id.clone().unwrap_or_default();
        // Fire-and-forget via a dedicated thread to avoid blocking glib loop.
        let device_id_clone = device_id.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().unwrap();
            rt.block_on(async move {
                if let Ok(conn) = Connection::session().await {
                    if let Ok(proxy) = TransportProxy::new(&conn).await {
                        let _ = proxy.send_packet(&device_id_clone,
                            r#"{"type":"disconnect"}"#).await;
                    }
                }
            });
        });
        // Update UI immediately — the server will confirm via signal.
        state_disc.borrow_mut().device_id = None;
        state_disc.borrow_mut().connected_at = None;
        name_row_disc.set_subtitle("Not connected");
        ip_row_disc.set_subtitle("—");
        uptime_row_disc.set_subtitle("—");
        status_badge_disc.set_label("Disconnected");
        status_badge_disc.set_css_classes(&["error"]);
        disconnect_btn_disc.set_sensitive(false);
    });

    // ── D-Bus signal subscriptions ────────────────────────────────────────────
    let state_sig   = state.clone();
    let name_row_s  = name_row.clone();
    let ip_row_s    = ip_row.clone();
    let status_s    = status_badge.clone();
    let disc_btn_s  = disconnect_btn.clone();
    let pkt_store_s = packet_store.clone();

    glib::spawn_future_local(async move {
        let conn = match Connection::session().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("ConnectionPage: D-Bus session failed: {e}");
                return;
            }
        };
        let proxy = match TransportProxy::new(&conn).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("ConnectionPage: Transport proxy failed: {e}");
                return;
            }
        };

        let mut connected_stream    = proxy.receive_device_connected().await.ok();
        let mut disconnected_stream = proxy.receive_device_disconnected().await.ok();
        let mut packet_stream       = proxy.receive_packet_received().await.ok();

        loop {
            tokio::select! {
                Some(signal) = async {
                    match connected_stream.as_mut() {
                        Some(s) => futures_util::StreamExt::next(s).await,
                        None => None,
                    }
                } => {
                    if let Ok(args) = signal.args() {
                        let mut s = state_sig.borrow_mut();
                        s.device_id    = Some(args.device_id.to_string());
                        s.device_name  = Some(args.name.to_string());
                        s.ip_addr      = Some(args.addr.to_string());
                        s.connected_at = Some(Instant::now());
                        drop(s);
                        name_row_s.set_subtitle(&args.name);
                        ip_row_s.set_subtitle(&args.addr);
                        status_s.set_label("Connected");
                        status_s.set_css_classes(&["success"]);
                        disc_btn_s.set_sensitive(true);
                    }
                }
                Some(signal) = async {
                    match disconnected_stream.as_mut() {
                        Some(s) => futures_util::StreamExt::next(s).await,
                        None => None,
                    }
                } => {
                    if let Ok(_args) = signal.args() {
                        let mut s = state_sig.borrow_mut();
                        s.device_id    = None;
                        s.connected_at = None;
                        drop(s);
                        name_row_s.set_subtitle("Not connected");
                        ip_row_s.set_subtitle("—");
                        status_s.set_label("Disconnected");
                        status_s.set_css_classes(&["error"]);
                        disc_btn_s.set_sensitive(false);
                    }
                }
                Some(signal) = async {
                    match packet_stream.as_mut() {
                        Some(s) => futures_util::StreamExt::next(s).await,
                        None => None,
                    }
                } => {
                    if let Ok(args) = signal.args() {
                        state_sig.borrow_mut().push_packet(args.json.to_string());
                        let n = pkt_store_s.n_items();
                        if n >= 50 { pkt_store_s.remove(0); }
                        pkt_store_s.append(&args.json);
                    }
                }
                else => break,
            }
        }
    });

    page
}
