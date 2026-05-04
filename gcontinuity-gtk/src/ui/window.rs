//! Main window — all pages and event handlers.
//!
//! FIXES:
//!   1. WindowHandle now holds device_name_label + device_status_label so
//!      on_device_connected() sets them dynamically (fixes "Android Device" bug).
//!   2. on_device_disconnected() updates status to "Disconnected" then navigates
//!      to PAGE_WAITING (fixes stays-on-device-page bug).
//!   3. build_waiting_page() now has Nearby Devices + Trusted Devices sections.
//!      WindowHandle holds nearby_group + trusted_group for population by app.rs.
//!   4. on_device_connected() clears the devices_group before adding new row
//!      to prevent duplicates on reconnect.

use adw::prelude::*;
use gtk4::glib;
use std::cell::RefCell;
use std::rc::Rc;

use super::devices_page::build_device_row;

pub const PAGE_WAITING:   &str = "waiting";
pub const PAGE_CONNECTED: &str = "connected";
pub const PAGE_DEVICE:    &str = "device";

#[derive(Clone)]
pub struct WindowHandle {
    pub stack:               gtk4::Stack,
    pub devices_group:       adw::PreferencesGroup,
    pub window:              adw::ApplicationWindow,
    pub refresh_btn:         gtk4::Button,
    // Home page sections
    pub nearby_group:        adw::PreferencesGroup,
    pub trusted_group:       adw::PreferencesGroup,
    // Device management page dynamic labels
    pub device_name_label:   gtk4::Label,
    pub device_status_label: gtk4::Label,
    // Tracks rows added to devices_group so we can remove them cleanly.
    // AdwPreferencesGroup.last_child() returns internal layout widgets —
    // calling remove() on those causes "tried to remove non-child" errors.
    pub connected_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
}

pub fn build_window(app: &adw::Application) -> (adw::ApplicationWindow, WindowHandle) {
    let hostname = std::fs::read_to_string("/etc/hostname")
        .unwrap_or_else(|_| "Linux Device\n".to_string())
        .trim().to_string();

    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title(&hostname)
        .default_width(520)
        .default_height(600)
        .resizable(false)
        .build();

    let header = adw::HeaderBar::new();

    let title_label = gtk4::Label::builder()
        .label(&hostname)
        .css_classes(["title"])
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .max_width_chars(28)
        .build();

    let edit_icon = gtk4::Image::builder()
        .icon_name("document-edit-symbolic")
        .pixel_size(14)
        .css_classes(["dim-label"])
        .build();

    let title_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .build();
    title_box.append(&title_label);
    title_box.append(&edit_icon);
    header.set_title_widget(Some(&title_box));

    let refresh_btn = gtk4::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Refresh")
        .css_classes(["flat"])
        .build();
    header.pack_start(&refresh_btn);

    let menu_btn = gtk4::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Settings")
        .css_classes(["flat"])
        .build();
    menu_btn.set_popover(Some(&build_settings_popover(&win)));
    header.pack_end(&menu_btn);

    let stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::SlideLeft)
        .transition_duration(280)
        .vexpand(true)
        .hexpand(true)
        .build();

    let (waiting_page, nearby_group, trusted_group) = build_waiting_page();
    stack.add_named(&waiting_page, Some(PAGE_WAITING));

    let (connected_page, devices_group) = build_connected_page();
    stack.add_named(&connected_page, Some(PAGE_CONNECTED));

    let (device_page, device_name_label, device_status_label) =
        build_device_management_page(&stack);
    stack.add_named(&device_page, Some(PAGE_DEVICE));

    stack.set_visible_child_name(PAGE_WAITING);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&stack));
    win.set_content(Some(&toolbar_view));

    let handle = WindowHandle {
        stack,
        devices_group,
        window: win.clone(),
        refresh_btn,
        nearby_group,
        trusted_group,
        device_name_label,
        device_status_label,
        connected_rows: Rc::new(RefCell::new(Vec::new())),
    };
    (win, handle)
}

// ── PAGE_WAITING ──────────────────────────────────────────────────────────────

fn build_waiting_page() -> (gtk4::Box, adw::PreferencesGroup, adw::PreferencesGroup) {
    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(20)
        .margin_top(20).margin_bottom(20)
        .margin_start(20).margin_end(20)
        .vexpand(true)
        .build();

    // ── Nearby Devices ────────────────────────────────────────────────────
    let nearby_group = adw::PreferencesGroup::builder()
        .title("Nearby Devices")
        .description("GContinuity devices found on your network")
        .build();

    // Placeholder shown when no nearby devices are found yet
    let nearby_placeholder = adw::ActionRow::builder()
        .title("Scanning…")
        .subtitle("Looking for devices on your network")
        .build();
    nearby_placeholder.add_prefix(
        &gtk4::Spinner::builder()
            .spinning(true)
            .valign(gtk4::Align::Center)
            .build(),
    );
    nearby_group.add(&nearby_placeholder);
    content_box.append(&nearby_group);

    // ── Trusted Devices ───────────────────────────────────────────────────
    let trusted_group = adw::PreferencesGroup::builder()
        .title("Trusted Devices")
        .description("Previously paired devices")
        .build();

    // Placeholder shown when no trusted devices exist
    let trusted_placeholder = adw::ActionRow::builder()
        .title("No paired devices")
        .subtitle("Pair a device to see it here")
        .build();
    trusted_placeholder.add_prefix(
        &gtk4::Image::builder()
            .icon_name("dialog-information-symbolic")
            .pixel_size(16)
            .css_classes(["dim-label"])
            .valign(gtk4::Align::Center)
            .build(),
    );
    trusted_group.add(&trusted_placeholder);
    content_box.append(&trusted_group);

    (wrap_in_scroll(content_box), nearby_group, trusted_group)
}

// ── PAGE_CONNECTED ────────────────────────────────────────────────────────────

fn build_connected_page() -> (gtk4::Box, adw::PreferencesGroup) {
    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(20)
        .margin_top(20).margin_bottom(20)
        .margin_start(20).margin_end(20)
        .vexpand(true)
        .build();

    let devices_group = adw::PreferencesGroup::builder()
        .title("Trusted Devices")
        .description("Paired devices and their connection status")
        .build();
    content_box.append(&devices_group);

    (wrap_in_scroll(content_box), devices_group)
}

// ── PAGE_DEVICE ───────────────────────────────────────────────────────────────

fn build_device_management_page(
    stack: &gtk4::Stack,
) -> (gtk4::Box, gtk4::Label, gtk4::Label) {
    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(20)
        .margin_top(20).margin_bottom(20)
        .margin_start(20).margin_end(20)
        .vexpand(true)
        .build();

    // ── Dynamic device info ───────────────────────────────────────────────
    let device_info_group = adw::PreferencesGroup::builder()
        .title("Connected Device")
        .build();

    // These labels are returned and updated dynamically by on_device_connected()
    let device_name_label = gtk4::Label::builder()
        .label("Android Device")
        .css_classes(["title-3"])
        .halign(gtk4::Align::Start)
        .build();

    let device_status_label = gtk4::Label::builder()
        .label("Connected")
        .css_classes(["success", "caption"])
        .halign(gtk4::Align::Start)
        .build();

    let name_row = adw::ActionRow::builder()
        .activatable(false)
        .build();

    let phone_icon = gtk4::Image::builder()
        .icon_name("phone-symbolic")
        .pixel_size(32)
        .valign(gtk4::Align::Center)
        .build();

    let text_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(2)
        .valign(gtk4::Align::Center)
        .hexpand(true)
        .build();
    text_box.append(&device_name_label);
    text_box.append(&device_status_label);

    let row_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .margin_top(12).margin_bottom(12)
        .margin_start(12).margin_end(12)
        .build();
    row_box.append(&phone_icon);
    row_box.append(&text_box);
    name_row.set_child(Some(&row_box));

    device_info_group.add(&name_row);
    content_box.append(&device_info_group);

    // ── Plugin management ─────────────────────────────────────────────────
    let plugins_group = adw::PreferencesGroup::builder()
        .title("Plugin Management")
        .description("Enable or disable features for this device")
        .build();

    for (title, subtitle, active) in [
        ("Clipboard Sync",         "Share clipboard between devices",        true),
        ("Notification Mirroring", "Mirror Android notifications on Linux",  true),
        ("File Transfer",          "Send and receive files",                 false),
    ] {
        plugins_group.add(&adw::SwitchRow::builder()
            .title(title).subtitle(subtitle).active(active).build());
    }
    content_box.append(&plugins_group);

    // ── Back button ───────────────────────────────────────────────────────
    let back_group = adw::PreferencesGroup::new();
    let back_row = adw::ActionRow::builder()
        .title("Back to devices")
        .activatable(true)
        .build();
    back_row.add_prefix(
        &gtk4::Image::builder()
            .icon_name("go-previous-symbolic")
            .pixel_size(16)
            .build(),
    );
    let stack_ref = stack.clone();
    back_row.connect_activated(move |_| {
        stack_ref.set_visible_child_name(PAGE_WAITING);
    });
    back_group.add(&back_row);
    content_box.append(&back_group);

    (wrap_in_scroll(content_box), device_name_label, device_status_label)
}

// ── Settings popover ──────────────────────────────────────────────────────────

fn build_settings_popover(win: &adw::ApplicationWindow) -> gtk4::Popover {
    let popover = gtk4::Popover::new();
    popover.set_position(gtk4::PositionType::Bottom);

    let vbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(2)
        .margin_top(4).margin_bottom(4)
        .margin_start(4).margin_end(4)
        .build();

    let make_btn = |label: &str, icon: &str| {
        let btn = gtk4::Button::builder().css_classes(["flat"]).build();
        let row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(10).margin_start(6).margin_end(6)
            .build();
        row.append(&gtk4::Image::builder().icon_name(icon).pixel_size(16).build());
        row.append(&gtk4::Label::builder().label(label).xalign(0.0).hexpand(true).build());
        btn.set_child(Some(&row));
        btn
    };

    let about_btn = make_btn("About GContinuity", "help-about-symbolic");
    let fp_btn    = make_btn("View Fingerprint",   "dialog-password-symbolic");
    let sep = gtk4::Separator::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .margin_top(4).margin_bottom(4).build();
    let quit_btn = make_btn("Quit", "application-exit-symbolic");

    let p1 = popover.clone();
    about_btn.connect_clicked(move |_| { p1.popdown(); });
    let p2 = popover.clone();
    fp_btn.connect_clicked(move |_| { p2.popdown(); });
    let p3 = popover.clone();
    let wr = win.clone();
    quit_btn.connect_clicked(move |_| { p3.popdown(); wr.close(); });

    vbox.append(&about_btn);
    vbox.append(&fp_btn);
    vbox.append(&sep);
    vbox.append(&quit_btn);
    popover.set_child(Some(&vbox));
    popover
}

// ── Public event handlers ─────────────────────────────────────────────────────

/// Called when DeviceConnected D-Bus signal fires.
/// Updates device name/status labels dynamically, navigates to PAGE_DEVICE.
pub fn on_device_connected(handle: &WindowHandle, device_json: &str) {
    let (name, fingerprint) = parse_device_json(device_json);

    // Update device management page labels dynamically
    handle.device_name_label.set_label(&name);
    handle.device_status_label.set_label("Connected");
    handle.device_status_label.remove_css_class("warning");
    handle.device_status_label.remove_css_class("dim-label");
    handle.device_status_label.add_css_class("success");

    // FIX: Remove previously tracked rows using stored Vec<ActionRow>.
    // NEVER use last_child()/remove() on AdwPreferencesGroup — it returns
    // internal layout widgets and causes "tried to remove non-child" crashes.
    {
        let mut rows = handle.connected_rows.borrow_mut();
        for row in rows.drain(..) {
            handle.devices_group.remove(&row);
        }
    }

    let stack_ref = handle.stack.clone();
    let row = build_device_row(
        &name, "Connected", &fingerprint, "Just now",
        move || { stack_ref.set_visible_child_name(PAGE_DEVICE); },
    );
    handle.devices_group.add(&row);
    // Track the row so we can remove it cleanly on next connect
    handle.connected_rows.borrow_mut().push(row);

    // Navigate: WAITING → CONNECTED (briefly) → DEVICE
    handle.stack.set_visible_child_name(PAGE_CONNECTED);
    let stack = handle.stack.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
        stack.set_visible_child_name(PAGE_DEVICE);
    });

    handle.window.present();
}

/// Called when DeviceDisconnected D-Bus signal fires.
/// Updates status label to "Disconnected" then navigates back to PAGE_WAITING.
pub fn on_device_disconnected(handle: &WindowHandle, _device_id: &str) {
    handle.device_status_label.set_label("Disconnected");
    handle.device_status_label.remove_css_class("success");
    handle.device_status_label.remove_css_class("warning");
    handle.device_status_label.add_css_class("dim-label");
    handle.stack.set_visible_child_name(PAGE_WAITING);
}

/// Add a device to the Nearby Devices section on the home page.
pub fn add_nearby_device(handle: &WindowHandle, name: &str, device_id: &str) {
    let row = adw::ActionRow::builder()
        .title(name).subtitle(device_id).activatable(false).build();
    row.add_prefix(&gtk4::Image::builder()
        .icon_name("phone-symbolic").pixel_size(20)
        .valign(gtk4::Align::Center).build());
    row.add_suffix(&gtk4::Image::builder()
        .icon_name("network-wireless-signal-good-symbolic").pixel_size(16)
        .css_classes(["dim-label"]).valign(gtk4::Align::Center).build());
    handle.nearby_group.add(&row);
}

/// Add a device to the Trusted Devices section on the home page.
/// Includes a remove (unpair) button on the right side of each row.
pub fn add_trusted_device(handle: &WindowHandle, name: &str, device_id: &str) {
    let row = adw::ActionRow::builder()
        .title(name).subtitle(device_id).activatable(false).build();
    row.add_prefix(&gtk4::Image::builder()
        .icon_name("channel-secure-symbolic").pixel_size(20)
        .valign(gtk4::Align::Center).build());

    // Remove button — lets users unpair devices from the home page
    let remove_btn = gtk4::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Remove trusted device")
        .css_classes(["flat", "destructive-action"])
        .valign(gtk4::Align::Center)
        .build();
    let row_ref   = row.clone();
    let group_ref = handle.trusted_group.clone();
    let id        = device_id.to_string();
    remove_btn.connect_clicked(move |_| {
        group_ref.remove(&row_ref);
        tracing::info!("Removed trusted device from UI: {id}");
        // TODO: call dbus_unpair_device(id.clone()) when daemon exposes the method
    });
    row.add_suffix(&remove_btn);
    handle.trusted_group.add(&row);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn wrap_in_scroll(content: gtk4::Box) -> gtk4::Box {
    let clamp = adw::Clamp::builder().maximum_size(460).child(&content).build();
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true).hexpand(true)
        .child(&clamp)
        .build();
    let page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .vexpand(true).hexpand(true)
        .build();
    page.append(&scroll);
    page
}

fn parse_device_json(json: &str) -> (String, String) {
    let name = extract_json_str(json, "name")
        .unwrap_or_else(|| "Android Device".to_string());
    let fp = extract_json_str(json, "fingerprint").unwrap_or_default();
    (name, fp)
}

fn extract_json_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let start  = json.find(&needle)?;
    let rest   = &json[start + needle.len()..];
    let colon  = rest.find(':')?;
    let after  = rest[colon + 1..].trim_start();
    if after.starts_with('"') {
        let inner = &after[1..];
        Some(inner[..inner.find('"')?].to_string())
    } else {
        None
    }
}
