use adw::prelude::*;

use super::devices_page::build_device_row;

pub fn build_window(app: &adw::Application) -> adw::ApplicationWindow {
    // Get hostname for this device section
    let hostname = std::fs::read_to_string("/etc/hostname")
        .unwrap_or_else(|_| "Linux Device\n".to_string())
        .trim()
        .to_string();


    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("GContinuity")
        .default_width(560)
        .default_height(600)
        .resizable(false)
        .build();

    // ── Toolbar view ──────────────────────────────────────────────────
    let toolbar_view = adw::ToolbarView::new();
    
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk4::Label::new(Some("GContinuity"))));
    toolbar_view.add_top_bar(&header);

    // ── Content ───────────────────────────────────────────────────────
    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(24)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();

    // ── This Device section ───────────────────────────────────────────
    let this_device_group = adw::PreferencesGroup::builder()
        .title("This Device")
        .build();

    let device_row = adw::ActionRow::builder()
        .title(&hostname)
        .subtitle("Your Linux device")
        .build();
    let computer_icon = gtk4::Image::from_icon_name("computer-symbolic");
    device_row.add_prefix(&computer_icon);
    this_device_group.add(&device_row);

    let fp_row = adw::ActionRow::builder()
        .title("Fingerprint")
        .subtitle("AA:BB:CC:DD:EE:FF:00:11:22:33")
        .build();
    let fp_icon = gtk4::Image::from_icon_name("dialog-password-symbolic");
    fp_row.add_prefix(&fp_icon);
    this_device_group.add(&fp_row);

    content_box.append(&this_device_group);

    // ── Paired Devices section ────────────────────────────────────────
    let paired_group = adw::PreferencesGroup::builder()
        .title("Paired Devices")
        .build();

    // Placeholder (no devices yet) — in real app this would be dynamic
    let placeholder = adw::StatusPage::builder()
        .icon_name("phone-symbolic")
        .title("No Paired Devices")
        .description("Open GContinuity on your Android phone to pair")
        .build();
    placeholder.set_hexpand(true);

    // Demo: comment this line and uncomment below to show a device row
    let has_devices = false;
    if has_devices {
        let row = build_device_row("Pixel 8 Pro", "connected", "AA:BB:CC:DD:EE:FF");
        paired_group.add(&row);
        content_box.append(&paired_group);
    } else {
        content_box.append(&paired_group);
        content_box.append(&placeholder);
    }

    // ── Clamp (max-width 480) ──────────────────────────────────────────
    let clamp = adw::Clamp::builder()
        .maximum_size(480)
        .child(&content_box)
        .build();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .child(&clamp)
        .build();

    toolbar_view.set_content(Some(&scroll));
    win.set_content(Some(&toolbar_view));

    win
}
