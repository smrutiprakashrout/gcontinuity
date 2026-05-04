#![allow(dead_code)] // Phase 1 — reactivated in Phase 3
//! Daemon configuration — XDG-compliant, stored at
//! `~/.config/gcontinuity/config.toml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: String,
    pub name: String,
    pub cert_sha256_hex: String,
}

impl PairedDevice {
    pub fn cert_sha256_bytes(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.cert_sha256_hex)
            .context("Invalid cert_sha256_hex in paired device")?;
        bytes.try_into().map_err(|_| anyhow::anyhow!("cert_sha256_hex must be 32 bytes"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // FIX: default changed from 37891 → 52000 to match mDNS advertisement
    // and Android firewall rule.
    pub port: u16,
    pub data_dir: PathBuf,
    pub device_name: String,
    pub paired_devices: Vec<PairedDevice>,
    pub download_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 52000, // ← was 37891
            data_dir: default_data_dir(),
            device_name: read_hostname(),
            paired_devices: Vec::new(),
            download_dir: default_download_dir(),
        }
    }
}

fn default_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gcontinuity")
}

fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Downloads")
        })
}

fn read_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn load_config() -> Result<Config> {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gcontinuity");

    std::fs::create_dir_all(&config_dir)
        .context("Failed to create config directory")?;

    let config_path = config_dir.join("config.toml");

    if config_path.exists() {
        let text = std::fs::read_to_string(&config_path)
            .context("Failed to read config.toml")?;
        let cfg: Config = toml::from_str(&text).context("Failed to parse config.toml")?;
        tracing::debug!("Loaded config from {}", config_path.display());
        Ok(cfg)
    } else {
        let defaults = Config::default();
        let text = toml::to_string_pretty(&defaults)
            .context("Failed to serialise default config")?;
        std::fs::write(&config_path, text)
            .context("Failed to write default config.toml")?;
        tracing::info!("Created default config at {}", config_path.display());
        Ok(defaults)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_port_is_52000() {
        assert_eq!(Config::default().port, 52000);
    }

    #[test]
    fn test_default_config_round_trips() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.port, parsed.port);
        assert_eq!(cfg.device_name, parsed.device_name);
    }

    #[test]
    fn test_paired_device_fingerprint_decode() {
        let pd = PairedDevice {
            device_id: "test".into(),
            name: "Phone".into(),
            cert_sha256_hex: hex::encode([0xABu8; 32]),
        };
        let bytes = pd.cert_sha256_bytes().unwrap();
        assert_eq!(bytes, [0xABu8; 32]);
    }
}
