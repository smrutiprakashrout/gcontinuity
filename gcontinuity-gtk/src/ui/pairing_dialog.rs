use adw::prelude::*;

/// Show a pairing confirmation dialog.
/// Returns true if the user clicked Accept, false if Reject/Cancel.
pub async fn show_pairing_dialog(
    parent: &impl IsA<gtk4::Widget>,
    device_name: &str,
    fingerprint: &str,
) -> bool {
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Pair with \"{}\"?", device_name))
        .body("Confirm that this fingerprint matches what appears on your Android phone.")
        .build();

    dialog.add_response("cancel", "Reject");
    dialog.add_response("accept", "Accept");
    dialog.set_response_appearance("accept", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("accept"));
    dialog.set_close_response("cancel");

    // Fingerprint label (monospace, blue, centered)
    let fp_label = gtk4::Label::builder()
        .label(fingerprint)
        .justify(gtk4::Justification::Center)
        .halign(gtk4::Align::Center)
        .css_classes(["fingerprint-label"])
        .build();

    let fp_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    fp_box.append(&fp_label);

    dialog.set_extra_child(Some(&fp_box));

    let response = dialog.choose_future(parent).await;
    response == "accept"
}
