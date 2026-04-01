use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Packet {
    Hello {
        device_id: String,
        name: String,
        version: u32,
        fingerprint: String,
    },
    PairRequest {
        device_id: String,
        name: String,
        fingerprint: String,
    },
    PairAccept {
        fingerprint: String,
    },
    PairReject {
        reason: String,
    },
    Ping {
        timestamp_ms: u64,
    },
    Pong {
        timestamp_ms: u64,
    },
    Disconnect {
        reason: String,
    },
}

impl Packet {
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_serialization() {
        let packet = Packet::Hello {
            device_id: "test_id".to_string(),
            name: "Test Device".to_string(),
            version: 1,
            fingerprint: "aa:bb".to_string(),
        };
        let json = packet.to_json().unwrap();
        assert!(json.contains(r#""type":"HELLO""#));
        assert!(json.contains(r#""device_id":"test_id""#));
    }

    #[test]
    fn test_ping_pong_roundtrip() {
        let packet = Packet::Ping { timestamp_ms: 12345 };
        let json = packet.to_json().unwrap();
        let decoded = Packet::from_json(&json).unwrap();
        match decoded {
            Packet::Ping { timestamp_ms } => assert_eq!(timestamp_ms, 12345),
            _ => panic!("Expected Ping"),
        }
    }
}
