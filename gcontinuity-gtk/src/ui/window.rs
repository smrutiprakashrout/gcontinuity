use adw::prelude::*;

use super::devices_page::build_device_row;

pub const PAGE_WAITING: &str = "waiting";
pub const PAGE_CONNECTED: &str = "connected";

#[derive(Clone)]
pub struct WindowHandle {
    pub stack: gtk4::Stack,
    pub devices_group: adw::PreferencesGroup,
    pub window: adw::ApplicationWindow,
}

pub fn build_window(app: &adw::Application) -> (adw::ApplicationWindow, WindowHandle) {
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

    let stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::SlideLeft)
        .transition_duration(300)
        .vexpand(true)
        .hexpand(true)
        .build();

    let waiting_page = build_waiting_page(&hostname);
    stack.add_named(&waiting_page, Some(PAGE_WAITING));

    let (connected_page, devices_group) = build_connected_page(&hostname);
    stack.add_named(&connected_page, Some(PAGE_CONNECTED));

    stack.set_visible_child_name(PAGE_WAITING);

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk4::Label::new(Some("GContinuity"))));
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&stack));

    win.set_content(Some(&toolbar_view));

    let handle = WindowHandle { stack, devices_group, window: win.clone() };
    (win, handle)
}

fn build_waiting_page(hostname: &str) -> gtk4::Box {
    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(24)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .vexpand(true)
        .build();

    let this_device_group = adw::PreferencesGroup::builder()
        .title("This Device")
        .build();

    let device_row = adw::ActionRow::builder()
        .title(hostname)
        .subtitle("Your Linux device")
        .build();
    device_row.add_prefix(&gtk4::Image::from_icon_name("computer-symbolic"));
    this_device_group.add(&device_row);

    let fp_row = adw::ActionRow::builder()
        .title("Fingerprint")
        .subtitle("AA:BB:CC:DD:EE:FF:00:11:22:33")
        .build();
    fp_row.add_prefix(&gtk4::Image::from_icon_name("dialog-password-symbolic"));
    this_device_group.add(&fp_row);

    content_box.append(&this_device_group);

    let status = adw::StatusPage::builder()
        .icon_name("phone-symbolic")
        .title("Waiting for Connection")
        .description("Open GContinuity on your Android phone to connect")
        .vexpand(true)
        .build();

    content_box.append(&status);

    let clamp = adw::Clamp::builder()
        .maximum_size(480)
        .child(&content_box)
        .build();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .child(&clamp)
        .build();

    let page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .vexpand(true)
        .hexpand(true)
        .build();
    page.append(&scroll);
    page
}

fn build_connected_page(hostname: &str) -> (gtk4::Box, adw::PreferencesGroup) {
    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(24)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .vexpand(true)
        .build();

    let this_device_group = adw::PreferencesGroup::builder()
        .title("This Device")
        .build();

    let device_row = adw::ActionRow::builder()
        .title(hostname)
        .subtitle("Your Linux device")
        .build();
    device_row.add_prefix(&gtk4::Image::from_icon_name("computer-symbolic"));
    this_device_group.add(&device_row);

    content_box.append(&this_device_group);

    let devices_group = adw::PreferencesGroup::builder()
        .title("Connected Devices")
        .description("Manage plugins and settings for each paired device")
        .build();

    content_box.append(&devices_group);

    let plugins_group = adw::PreferencesGroup::builder()
        .title("Plugin Management")
        .description("Enable or disable features for connected devices")
        .build();

    let clipboard_row = adw::SwitchRow::builder()
        .title("Clipboard Sync")
        .subtitle("Share clipboard between devices")
        .active(true)
        .build();
    plugins_group.add(&clipboard_row);

    let notification_row = adw::SwitchRow::builder()
        .title("Notification Mirroring")
        .subtitle("Mirror Android notifications on this device")
        .active(true)
        .build();
    plugins_group.add(&notification_row);

    let file_transfer_row = adw::SwitchRow::builder()
        .title("File Transfer")
        .subtitle("Send and receive files")
        .active(false)
        .build();
    plugins_group.add(&file_transfer_row);

    content_box.append(&plugins_group);

    let clamp = adw::Clamp::builder()
        .maximum_size(480)
        .child(&content_box)
        .build();

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .child(&clamp)
        .build();

    let page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .vexpand(true)
        .hexpand(true)
        .build();
    page.append(&scroll);

    (page, devices_group)
}

pub fn on_device_connected(handle: &WindowHandle, device_json: &str) {
    let (name, fingerprint) = parse_device_json(device_json);
    let row = build_device_row(&name, "connected", &fingerprint);
    handle.devices_group.add(&row);
    handle.stack.set_visible_child_name(PAGE_CONNECTED);
    handle.window.present();
}

pub fn on_device_disconnected(handle: &WindowHandle, _device_id: &str) {
    handle.stack.set_visible_child_name(PAGE_WAITING);
}

fn parse_device_json(json: &str) -> (String, String) {
    let name = extract_json_str(json, "name")
        .unwrap_or_else(|| "Android Device".to_string());
    let fp = extract_json_str(json, "fingerprint")
        .unwrap_or_else(|| "AA:BB:CC:DD".to_string());
    (name, fp)
}

fn extract_json_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let start = json.find(&needle)?;
    let after_key = &json[start + needle.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    if after_colon.starts_with('"') {
        let inner = &after_colon[1..];
        let end = inner.find('"')?;
        Some(inner[..end].to_string())
    } else {
        None
    }
}
