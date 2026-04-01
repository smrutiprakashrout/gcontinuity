// App state and D-Bus connectivity placeholder
pub struct AppState {
    pub device_name: String,
    pub fingerprint: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            device_name: read_hostname(),
            fingerprint: "AA:BB:CC:DD:EE:FF".to_string(),
        }
    }
}

pub fn read_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
        .chars()
        .collect::<String>()
        .into()
}
