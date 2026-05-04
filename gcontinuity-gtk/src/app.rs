//! GTK application entry point.
//!
//! FIXES:
//!   1. dbus_accept_pairing / dbus_reject_pairing now make real D-Bus calls
//!      that unblock the daemon's handshake PairingGate.
//!   2. Trusted devices section populated at startup via list_trusted_devices().
//!   3. on_connected callback passes real device name to on_device_connected().
//!   4. on_disconnected navigates to PAGE_WAITING and updates status label.

use adw::prelude::*;
use gtk4::glib;

use crate::dbus::{
    dbus_accept_pairing, dbus_reject_pairing,
    query_initial_state, query_trusted_devices, InitialState,
    start_dbus_listener,
};
use crate::ui::pairing_dialog::{show_pairing_dialog, PairingInfo};
use crate::ui::window::{
    build_window, on_device_connected, on_device_disconnected,
    add_trusted_device,
};

pub fn read_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default().trim().to_string()
}

pub fn on_activate(app: &adw::Application) {
    let (win, handle) = build_window(app);
    win.present();

    // ── Refresh button ────────────────────────────────────────────────────
    let refresh_btn = handle.refresh_btn.clone();
    refresh_btn.connect_clicked(move |btn| {
        btn.set_icon_name("process-working-symbolic");
        btn.set_sensitive(false);
        let b = btn.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1000), move || {
            b.set_icon_name("view-refresh-symbolic");
            b.set_sensitive(true);
        });
        tracing::info!("Refresh triggered");
    });

    // ── Populate Trusted Devices section on home page at startup ──────────
    let handle_trusted = handle.clone();
    query_trusted_devices(move |devices| {
        for (device_id, name) in devices {
            add_trusted_device(&handle_trusted, &name, &device_id);
        }
    });

    // ── Query daemon state at startup ─────────────────────────────────────
    let handle_init = handle.clone();
    query_initial_state(move |state| {
        match state {
            InitialState::Connected(device_id, name) => {
                tracing::info!("Daemon already connected to {name}");
                let json = format!(
                    r#"{{"device_id":"{device_id}","name":"{name}","fingerprint":""}}"#
                );
                on_device_connected(&handle_init, &json);
            }
            InitialState::Waiting => {
                tracing::info!("Daemon running, no device — on waiting page");
            }
            InitialState::DaemonNotRunning => {
                tracing::warn!("Daemon not on D-Bus — on waiting page");
            }
        }
    });

    // ── Live D-Bus signal listeners ───────────────────────────────────────
    let handle_pairing      = handle.clone();
    let handle_connected    = handle.clone();
    let handle_disconnected = handle.clone();

    start_dbus_listener(
        // on_pairing_requested — show dialog, wire Accept/Reject to real D-Bus calls
        move |device_id, name, fingerprint| {
            tracing::info!("PairingRequested from {name} — showing dialog");
            let info = PairingInfo { device_id: device_id.clone(), name, fingerprint };
            let accept_id = device_id.clone();
            let reject_id = device_id.clone();
            show_pairing_dialog(
                &handle_pairing.window,
                info,
                move |_| dbus_accept_pairing(accept_id.clone()),
                move |_| dbus_reject_pairing(reject_id.clone()),
            );
        },

        // on_connected — update dynamic device name, navigate to device page
        move |_device_id, name, _addr| {
            tracing::info!("DeviceConnected: {name}");
            let json = format!(r#"{{"name":"{name}","fingerprint":""}}"#);
            on_device_connected(&handle_connected, &json);
        },

        // on_disconnected — update status label, navigate to waiting page
        move |device_id| {
            tracing::info!("DeviceDisconnected: {device_id}");
            on_device_disconnected(&handle_disconnected, &device_id);
        },

        // on_pairing_accepted — daemon stored the device, update trusted list
        move |device_id| {
            tracing::info!("PairingAccepted: {device_id}");
            // DeviceConnected signal will fire immediately after — it handles navigation
        },

        // on_pairing_rejected
        move |device_id| {
            tracing::warn!("PairingRejected: {device_id}");
        },
    );
}
