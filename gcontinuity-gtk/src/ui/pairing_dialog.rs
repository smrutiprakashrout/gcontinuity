use adw::prelude::*;

/// Information about the device requesting pairing.
/// FIXED: fields are now passed directly from D-Bus signal args,
/// not parsed from a JSON string (which was fragile and unnecessary).
#[derive(Debug, Clone)]
pub struct PairingInfo {
    pub device_id:   String,
    pub name:        String,
    pub fingerprint: String,
}

/// Show the pairing confirmation dialog.
///
/// `on_accept` — called on the glib main thread if the user clicks Accept.
/// `on_reject` — called on the glib main thread if the user clicks Reject.
pub fn show_pairing_dialog(
    parent:    &adw::ApplicationWindow,
    info:      PairingInfo,
    on_accept: impl Fn(String) + 'static,
    on_reject: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Pair with {}?", info.name))
        .body("Compare the fingerprint below with what appears on your Android device. Accept only if they match.")
        .build();

    dialog.add_response("reject", "Reject");
    dialog.add_response("accept", "Accept");
    dialog.set_response_appearance("accept", adw::ResponseAppearance::Suggested);
    dialog.set_response_appearance("reject", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("reject"));
    dialog.set_close_response("reject");

    // ── Fingerprint display ───────────────────────────────────────────────
    let fp_display = format_fingerprint_two_lines(&info.fingerprint);

    let fp_value = gtk4::Label::builder()
        .label(&fp_display)
        .halign(gtk4::Align::Center)
        .justify(gtk4::Justification::Center)
        .selectable(true)
        .wrap(true)
        .css_classes(["monospace", "title-3"])
        .build();

    let fp_label = gtk4::Label::builder()
        .label("Security fingerprint")
        .halign(gtk4::Align::Center)
        .css_classes(["caption", "dim-label"])
        .build();

    let card = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(4)
        .margin_start(8).margin_end(8)
        .margin_top(4).margin_bottom(4)
        .css_classes(["card"])
        .build();
    card.append(&fp_label);
    card.append(&fp_value);

    let device_row = adw::ActionRow::builder()
        .title(&info.name)
        .subtitle("Android device requesting pairing")
        .build();
    device_row.add_prefix(
        &gtk4::Image::builder()
            .icon_name("phone-symbolic")
            .pixel_size(32)
            .build(),
    );

    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(8)
        .build();
    content_box.append(&device_row);
    content_box.append(&card);

    dialog.set_extra_child(Some(&content_box));

    // ── Response handler ──────────────────────────────────────────────────
    let accept_id = info.device_id.clone();
    let reject_id = info.device_id.clone();

    dialog.connect_response(None, move |_dlg, response| {
        if response == "accept" {
            on_accept(accept_id.clone());
        } else {
            on_reject(reject_id.clone());
        }
    });

    dialog.present(Some(parent));
}

fn format_fingerprint_two_lines(fp: &str) -> String {
    let parts: Vec<&str> = fp.split(':').collect();
    if parts.is_empty() {
        return fp.to_string();
    }
    let half = (parts.len() + 1) / 2;
    let (first, second) = parts.split_at(half.min(parts.len()));
    if second.is_empty() {
        first.join(":")
    } else {
        format!("{}\n{}", first.join(":"), second.join(":"))
    }
}
