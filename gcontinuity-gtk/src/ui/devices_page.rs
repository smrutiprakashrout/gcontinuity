use adw::prelude::*;

/// Build a device row — KDE Connect style:
///
///  ┌─────────────────────────────────────────────────────┐
///  │  [phone icon]  Device Name          Connected  [⊗]  │
///  │                13:79:75:…  ·  Just now               │
///  └─────────────────────────────────────────────────────┘
///
/// Parameters:
///   name           — device display name
///   status         — "Connected" | "Disconnected" | "Reconnecting"
///   fingerprint    — full fingerprint string (will be truncated)
///   last_connected — human-readable time string e.g. "Just now"
///   on_click       — called when the row is activated (navigate to mgmt page)
pub fn build_device_row(
    name: &str,
    status: &str,
    fingerprint: &str,
    last_connected: &str,
    on_click: impl Fn() + 'static,
) -> adw::ActionRow {
    let fp_short = truncate_fingerprint(fingerprint);
    let subtitle  = format!("{fp_short}  ·  {last_connected}");

    let row = adw::ActionRow::builder()
        .title(name)
        .subtitle(&subtitle)
        .activatable(true)          // makes the whole row clickable
        .build();

    // ── Phone icon (prefix) ───────────────────────────────────────────────
    row.add_prefix(
        &gtk4::Image::builder()
            .icon_name("phone-symbolic")
            .pixel_size(20)
            .valign(gtk4::Align::Center)
            .build(),
    );

    // ── Status text label (suffix) — KDE Connect style ────────────────────
    // "Connected"     → Adwaita success colour (green)
    // "Reconnecting"  → warning colour (amber via .warning css class)
    // anything else   → dim grey
    let status_label = gtk4::Label::builder()
        .label(status)
        .valign(gtk4::Align::Center)
        .build();

    match status {
        "Connected"    => status_label.add_css_class("success"),
        "Reconnecting" => status_label.add_css_class("warning"),
        _              => status_label.add_css_class("dim-label"),
    }

    row.add_suffix(&status_label);

    // ── Unpair button (suffix) ────────────────────────────────────────────
    let unpair_btn = gtk4::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Unpair device")
        .valign(gtk4::Align::Center)
        .css_classes(["flat"])
        .build();

    // Prevent the unpair button click from also firing the row's activate.
    unpair_btn.connect_clicked(|_| {
        // TODO: dbus_unpair_device(device_id.clone());
    });

    row.add_suffix(&unpair_btn);

    // ── Row activated → navigate to device management page ────────────────
    row.connect_activated(move |_| on_click());

    row
}

fn truncate_fingerprint(fp: &str) -> String {
    let parts: Vec<&str> = fp.split(':').take(3).collect();
    if parts.len() == 3 {
        format!("{}:…", parts.join(":"))
    } else {
        fp.to_string()
    }
}
