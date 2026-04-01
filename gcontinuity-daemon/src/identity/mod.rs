use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

pub struct Identity {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint: String,
    pub device_id: String,
}

fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gcontinuity")
}

impl Identity {
    pub async fn load_or_create(device_name: &str) -> Result<Identity> {
        let dir = data_dir();
        fs::create_dir_all(&dir).await?;

        let cert_path = dir.join("identity.crt");
        let key_path = dir.join("identity.key");
        let config_path = dir.join("config.toml"); // Simple config for device_id

        if cert_path.exists() && key_path.exists() {
            let cert_pem = fs::read_to_string(&cert_path).await?;
            let key_pem = fs::read_to_string(&key_path).await?;
            
            // Re-parse cert to get DER
            let mut certs_reader = std::io::BufReader::new(cert_pem.as_bytes());
            let certs = rustls_pemfile::certs(&mut certs_reader).collect::<Result<Vec<_>, _>>()?;
            let der = &certs[0];
            let fingerprint = Self::compute_fingerprint(der);
            
            let device_id = if config_path.exists() {
                let config_str = fs::read_to_string(&config_path).await?;
                // basic parsing just to get ID
                config_str.lines()
                    .find(|l| l.starts_with("device_id = "))
                    .and_then(|l| l.split('"').nth(1))
                    .map(String::from)
                    .unwrap_or_else(|| Uuid::new_v4().to_string())
            } else {
                Uuid::new_v4().to_string()
            };

            // Ensure config exists if it was missing
            if !config_path.exists() {
                fs::write(&config_path, format!("device_id = \"{}\"\n", device_id)).await?;
            }

            return Ok(Identity {
                cert_pem,
                key_pem,
                fingerprint,
                device_id,
            });
        }

        // Generate new — rcgen 0.13 returns CertifiedKey { cert, key_pair }
        let certified = rcgen::generate_simple_self_signed(vec![device_name.to_string()])?;
        let cert_pem = certified.cert.pem();
        let key_pem = certified.key_pair.serialize_pem();
        let der: &[u8] = certified.cert.der();
        let fingerprint = Self::compute_fingerprint(der);
        let device_id = Uuid::new_v4().to_string();

        fs::write(&cert_path, &cert_pem).await?;
        fs::write(&key_path, &key_pem).await?;
        fs::write(&config_path, format!("device_id = \"{}\"\n", device_id)).await?;

        Ok(Identity {
            cert_pem,
            key_pem,
            fingerprint,
            device_id,
        })
    }

    fn compute_fingerprint(der: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(der);
        let result = hasher.finalize();
        let hex_str = hex::encode(result);
        
        let chars: Vec<char> = hex_str.chars().collect();
        let pairs: Vec<String> = chars
            .chunks(2)
            .map(|chunk| chunk.iter().collect::<String>())
            .collect();
            
        pairs.join(":")
    }
}
