use adw::prelude::*;

/// "This Device" page showing device name and fingerprint.
pub fn build_this_device_group(hostname: &str, fingerprint: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("This Device")
        .build();

    let device_row = adw::ActionRow::builder()
        .title(hostname)
        .subtitle("Your Linux device")
        .build();
    device_row.add_prefix(&gtk4::Image::from_icon_name("computer-symbolic"));
    group.add(&device_row);

    let fp_row = adw::ActionRow::builder()
        .title("Fingerprint")
        .subtitle(fingerprint)
        .build();
    fp_row.add_prefix(&gtk4::Image::from_icon_name("dialog-password-symbolic"));
    group.add(&fp_row);

    group
}
