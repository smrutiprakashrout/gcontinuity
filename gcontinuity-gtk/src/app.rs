use adw::prelude::*;

use crate::dbus::start_dbus_listener;
use crate::ui::window::{build_window, on_device_connected};

pub struct AppState {
    pub device_name: String,
    pub fingerprint: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            device_name: read_hostname(),
            fingerprint: "AA:BB:CC:DD:EE:FF".to_string(),
        }
    }
}

pub fn read_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn on_activate(app: &adw::Application) {
    let (win, handle) = build_window(app);
    win.present();

    // `glib::MainContext::channel` delivers signals on the main thread, so the
    // closures here are never called from a background thread — GTK objects are
    // safe to use directly. No Arc/Mutex/Rc needed.
    start_dbus_listener(
        move |device_json| on_device_connected(&handle, &device_json),
        move |_device_id|  { /* no-op on disconnect for now */ },
    );
}
