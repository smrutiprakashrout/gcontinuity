mod app;
mod ui;
mod dbus;

use adw::prelude::*;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let app = adw::Application::builder()
        .application_id("org.gcontinuity.Settings")
        .build();

    app.connect_activate(|app| {
        let win = ui::window::build_window(app);
        win.present();
    });

    // Load custom CSS
    app.connect_startup(|_| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(
            r#"
            .fingerprint-label {
                font-family: monospace;
                font-size: 15px;
                font-weight: bold;
                letter-spacing: 2px;
                color: #007AFF;
                padding: 16px;
            }
            .status-dot {
                min-width: 10px;
                min-height: 10px;
                border-radius: 5px;
            }
            "#,
        );
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("Could not connect to a display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });

    app.run();
}
