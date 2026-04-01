use adw::prelude::*;

/// Build a device row with:
/// - Colored status dot (prefix)
/// - Device name + truncated fingerprint
/// - Unpair button (suffix)
pub fn build_device_row(name: &str, state: &str, fingerprint: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(name)
        .subtitle(truncate_fingerprint(fingerprint))
        .activatable(false)
        .build();

    // Status dot using DrawingArea
    let dot_color = match state {
        "connected"    => (0x34_u8, 0xC7_u8, 0x59_u8), // Apple green
        "reconnecting" => (0xFF_u8, 0x95_u8, 0x00_u8), // Apple amber
        _              => (0x8E_u8, 0x8E_u8, 0x93_u8), // Apple grey
    };
    let (r, g, b) = dot_color;

    let dot = gtk4::DrawingArea::builder()
        .width_request(10)
        .height_request(10)
        .valign(gtk4::Align::Center)
        .margin_end(4)
        .build();

    dot.set_draw_func(move |_da, cr, w, h| {
        cr.arc(w as f64 / 2.0, h as f64 / 2.0, w as f64 / 2.0, 0.0, 2.0 * std::f64::consts::PI);
        cr.set_source_rgb(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
        let _ = cr.fill();
    });

    row.add_prefix(&dot);

    // Unpair button
    let unpair_btn = gtk4::Button::builder()
        .icon_name("network-wireless-disconnected-symbolic")
        .tooltip_text("Unpair")
        .valign(gtk4::Align::Center)
        .css_classes(["flat"])
        .build();
    unpair_btn.connect_clicked(|_| {
        // TODO: call D-Bus UnpairDevice
    });
    row.add_suffix(&unpair_btn);

    row
}

fn truncate_fingerprint(fp: &str) -> String {
    let parts: Vec<&str> = fp.split(':').take(3).collect();
    if parts.len() == 3 {
        format!("{}:...", parts.join(":"))
    } else {
        fp.to_string()
    }
}
