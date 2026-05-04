// gcontinuity-common/src/packet.rs
//
// WIRE PROTOCOL CONTRACT — do NOT change variant names or field names.
// Every variant here is part of the Phase 1 pairing handshake.
// Phase 2–6 transport packets live in gcontinuity-daemon/src/transport/packet.rs
// and are fully handled by the daemon only (not shared with the GTK crate).
//
// CHANGE FROM OLD VERSION:
//   - `rename_all` was "SCREAMING_SNAKE_CASE" → changed to "snake_case"
//     so {"type":"HELLO"} becomes {"type":"hello"}, matching Android.
//   - `Hello` no longer carries `fingerprint` — Android does not send
//     it in Hello; fingerprint is exchanged in PairRequest/PairAccept.
//   - `Ping`/`Pong` no longer carry `timestamp_ms` — Android's
//     TransportManager sends bare `{"type":"ping"}` / `{"type":"pong"}`.
//   - `Disconnect` no longer carries `reason` — Android sends bare
//     `{"type":"disconnect"}`.
//   - to_json() now returns String (infallible), not serde_json::Result<String>,
//     matching the daemon transport packet API style.

use serde::{Deserialize, Serialize};

/// Packets exchanged during the Phase 1 pairing ceremony and initial
/// connection handshake.
///
/// Serialised as `{"type":"<snake_case_variant>", ...}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Packet {
    /// First packet sent by Android after TLS handshake completes.
    /// Linux replies with its own Hello.
    Hello {
        device_id: String,
        name: String,
        version: u32,
    },

    /// Android requests pairing; carries the fingerprint that the user
    /// must verify visually on both screens.
    PairRequest {
        device_id: String,
        name: String,
        fingerprint: String,
    },

    /// Linux (or Android) accepts the pairing; echoes the fingerprint
    /// so the other side can store it.
    PairAccept {
        fingerprint: String,
    },

    /// Either side rejects or cancels pairing.
    PairReject {
        reason: String,
    },

    /// Keepalive probe. Receiver must reply with Pong.
    Ping,

    /// Reply to Ping.
    Pong,

    /// Graceful connection teardown.
    Disconnect,
}

impl Packet {
    /// Serialise to JSON. Always succeeds — every field is JSON-compatible.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Packet is always JSON-serialisable")
    }

    /// Parse from a JSON string. Returns an error for unknown or malformed input.
    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_roundtrip() {
        let p = Packet::Hello { device_id: "id1".into(), name: "Phone".into(), version: 1 };
        let json = p.to_json();
        // Discriminator must be snake_case to match Android kotlinx.serialization
        assert!(json.contains(r#""type":"hello""#), "got: {json}");
        let decoded: Packet = serde_json::from_str(&json).unwrap();
        assert_eq!(p, decoded);
    }

    #[test]
    fn pair_request_roundtrip() {
        let p = Packet::PairRequest {
            device_id: "id1".into(),
            name: "Phone".into(),
            fingerprint: "ab:cd:ef".into(),
        };
        let json = p.to_json();
        assert!(json.contains(r#""type":"pair_request""#), "got: {json}");
        let decoded: Packet = serde_json::from_str(&json).unwrap();
        assert_eq!(p, decoded);
    }

    #[test]
    fn ping_pong_bare() {
        // Must be bare {"type":"ping"} with no extra fields
        let ping = Packet::Ping;
        let json = ping.to_json();
        assert_eq!(json, r#"{"type":"ping"}"#, "got: {json}");
        let decoded: Packet = serde_json::from_str(&json).unwrap();
        assert_eq!(ping, decoded);
    }

    #[test]
    fn disconnect_bare() {
        let p = Packet::Disconnect;
        let json = p.to_json();
        assert_eq!(json, r#"{"type":"disconnect"}"#, "got: {json}");
    }

    #[test]
    fn unknown_type_is_error() {
        assert!(Packet::from_json(r#"{"type":"HELLO"}"#).is_err(), "SCREAMING_SNAKE must fail");
        assert!(Packet::from_json(r#"{"type":"unknown_xyz"}"#).is_err());
    }

    #[test]
    fn hello_missing_name_is_error() {
        // `name` is required — missing field must error, not panic
        assert!(Packet::from_json(r#"{"type":"hello","device_id":"id1","version":1}"#).is_err());
    }
}
