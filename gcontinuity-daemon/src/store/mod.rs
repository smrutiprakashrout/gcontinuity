#![allow(dead_code)] // Phase 1 — reactivated in Phase 3
use anyhow::Result;
use gcontinuity_common::DeviceInfo;
use sled::Db;
use std::path::PathBuf;

pub struct PeerStore {
    db: Db,
}

impl PeerStore {
    pub fn open() -> Result<Self> {
        let dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gcontinuity");
            
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join("peers.db");
        
        let db = sled::open(db_path)?;
        Ok(Self { db })
    }

    pub fn is_trusted(&self, device_id: &str) -> bool {
        self.db.contains_key(device_id).unwrap_or(false)
    }

    pub fn get_fingerprint(&self, device_id: &str) -> Option<String> {
        self.db.get(device_id)
            .ok()
            .flatten()
            .and_then(|bytes| {
                // Return device info fingerprint
                serde_json::from_slice::<DeviceInfo>(&bytes)
                    .map(|d| d.fingerprint)
                    .ok()
            })
    }

    pub fn store_device(&self, device: &DeviceInfo) -> Result<()> {
        let key = device.device_id.as_bytes();
        let value = serde_json::to_vec(device)?;
        self.db.insert(key, value)?;
        self.db.flush()?;
        Ok(())
    }

    pub fn remove_device(&self, device_id: &str) -> Result<()> {
        self.db.remove(device_id)?;
        self.db.flush()?;
        Ok(())
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let mut devices = Vec::new();
        // .flatten() skips any IO errors — we log deserialization failures only.
        for (_k, v) in self.db.iter().flatten() {
            match serde_json::from_slice::<DeviceInfo>(&v) {
                Ok(device) => devices.push(device),
                Err(_) => tracing::warn!("Failed to parse device entry in DB"),
            }
        }
        Ok(devices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_store() {
        let dir = std::env::temp_dir().join(format!("gcontinuity-test-{}", Uuid::new_v4()));
        let db = sled::open(&dir).unwrap();
        let store = PeerStore { db };

        let device = DeviceInfo {
            device_id: "test_id_1".to_string(),
            name: "Test Phone".to_string(),
            fingerprint: "00:11".to_string(),
            version: 1,
        };

        store.store_device(&device).unwrap();
        assert!(store.is_trusted("test_id_1"));
        assert_eq!(store.get_fingerprint("test_id_1").unwrap(), "00:11");
        
        let devices = store.list_devices().unwrap();
        assert_eq!(devices.len(), 1);
        
        store.remove_device("test_id_1").unwrap();
        assert!(!store.is_trusted("test_id_1"));
    }
}
