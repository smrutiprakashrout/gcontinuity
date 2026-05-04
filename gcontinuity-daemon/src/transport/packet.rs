//! Full typed packet enum — every variant for Phases 2–6.
//!
//! Defined in full now so the enum shape never changes (which would break
//! the wire protocol).  Phase 3–6 variants are carried through the router
//! to the feature layer via `FeatureEvent`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Sub-types ────────────────────────────────────────────────────────────────

/// Actions the Linux side can send to control media playback on Android.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MediaAction {
    Play,
    Pause,
    Next,
    Previous,
    SeekTo { ms: u64 },
    VolumeSet { pct: u8 },
}

/// Categories of raw input events forwarded from the Android touch/keyboard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    MouseMove,
    MouseButton,
    MouseScroll,
    KeyPress,
    KeyRelease,
}

// ── Main enum ────────────────────────────────────────────────────────────────

/// Every packet type that will exist across Phases 2–6.
///
/// `serde(tag = "type")` produces `{"type":"hello", ...}` on the wire.
/// `rename_all = "snake_case"` keeps the tag values lower-snake for readability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Packet {
    // ── Phase 2: Handshake ────────────────────────────────────────────────
    /// First packet sent by the Android peer after TLS handshake.
    Hello { device_id: String, name: String, version: u32 },
    /// Server acknowledges a valid Hello from a trusted peer.
    Ack,
    /// Keepalive probe sent every 30 s of silence.
    Ping,
    /// Response to Ping — resets the keepalive timer.
    Pong,
    /// Peer requests to resume a previous session using its stored token.
    SessionResume { session_token: String },
    /// Graceful connection teardown.
    Disconnect,

    // ── Phase 3: Data ────────────────────────────────────────────────────
    ClipboardSync    { mime: String, data: String },
    BatteryUpdate    { percent: u8, charging: bool },
    FileSendOffer    { file_id: String, name: String, size: u64, mime: String },
    FileSendAccept   { file_id: String },
    FileSendReject   { file_id: String },
    FileSendEof      { file_id: String, sha256: String },
    FileProgress     { file_id: String, bytes_done: u64, total: u64 },

    // ── Phase 4: Sync ────────────────────────────────────────────────────
    NotificationPost    { id: u64, app: String, title: String, body: String, icon_b64: Option<String> },
    NotificationDismiss { id: u64 },
    NotificationReply   { id: u64, text: String },
    ObsidianFileDelta   { path: String, hash: String, data_b64: String },
    MediaStateUpdate    { title: String, artist: String, album: String, playing: bool, position_ms: u64, duration_ms: u64 },
    MediaCommand        { action: MediaAction },

    // ── Phase 5: Control ─────────────────────────────────────────────────
    /// Raw pointer / keyboard input forwarded from Android.
    InputEvent          { kind: InputKind, data: Value },
    RunCommandRequest   { command_id: String },
    RunCommandOutput    { command_id: String, stdout: String, stderr: String, exit_code: i32 },
    ScreenShareStart,
    ScreenShareStop,

    // ── Phase 6: Experimental ────────────────────────────────────────────
    WebcamStart,
    WebcamStop,

    // ── WebRTC Signaling (all phases) ────────────────────────────────────
    WebRtcSdpOffer     { session_id: String, sdp: String },
    WebRtcSdpAnswer    { session_id: String, sdp: String },
    WebRtcIceCandidate { session_id: String, candidate: String, sdp_mid: String, sdp_m_line_index: u32 },
    WebRtcClose        { session_id: String },
}

impl Packet {
    /// Serialise to JSON.  A `Packet` is always serialisable because every
    /// field type is JSON-compatible.  The `expect` here is the only
    /// intentional one in daemon code — a serialisation failure would be a
    /// programming error, not a runtime condition.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Packet is always JSON-serialisable")
    }

    /// Parse from a JSON string, returning an error for unknown/malformed input.
    pub fn from_json(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip every variant through to_json / from_json.
    macro_rules! roundtrip {
        ($name:ident, $packet:expr) => {
            #[test]
            fn $name() {
                let pkt: Packet = $packet;
                let json = pkt.to_json();
                let decoded = Packet::from_json(&json).expect("round-trip must succeed");
                assert_eq!(pkt, decoded);
            }
        };
    }

    roundtrip!(rt_hello,     Packet::Hello { device_id: "id1".into(), name: "Phone".into(), version: 2 });
    roundtrip!(rt_ack,       Packet::Ack);
    roundtrip!(rt_ping,      Packet::Ping);
    roundtrip!(rt_pong,      Packet::Pong);
    roundtrip!(rt_session_resume, Packet::SessionResume { session_token: "tok".into() });
    roundtrip!(rt_disconnect, Packet::Disconnect);

    roundtrip!(rt_clipboard, Packet::ClipboardSync { mime: "text/plain".into(), data: "hello".into() });
    roundtrip!(rt_battery,   Packet::BatteryUpdate { percent: 80, charging: true });
    roundtrip!(rt_file_offer, Packet::FileSendOffer { file_id: "f1".into(), name: "a.txt".into(), size: 1024, mime: "text/plain".into() });
    roundtrip!(rt_file_accept, Packet::FileSendAccept { file_id: "f1".into() });
    roundtrip!(rt_file_reject, Packet::FileSendReject { file_id: "f1".into() });
    roundtrip!(rt_file_eof,   Packet::FileSendEof { file_id: "f1".into(), sha256: "abc123".into() });
    roundtrip!(rt_file_progress, Packet::FileProgress { file_id: "f1".into(), bytes_done: 512, total: 1024 });

    roundtrip!(rt_notif_post, Packet::NotificationPost { id: 1, app: "Signal".into(), title: "Hi".into(), body: "Hello".into(), icon_b64: None });
    roundtrip!(rt_notif_dismiss, Packet::NotificationDismiss { id: 1 });
    roundtrip!(rt_notif_reply, Packet::NotificationReply { id: 1, text: "ok".into() });
    roundtrip!(rt_obsidian, Packet::ObsidianFileDelta { path: "note.md".into(), hash: "h1".into(), data_b64: "YQ==".into() });
    roundtrip!(rt_media_state, Packet::MediaStateUpdate { title: "Song".into(), artist: "A".into(), album: "B".into(), playing: true, position_ms: 12000, duration_ms: 240000 });
    roundtrip!(rt_media_cmd_play, Packet::MediaCommand { action: MediaAction::Play });
    roundtrip!(rt_media_cmd_seek, Packet::MediaCommand { action: MediaAction::SeekTo { ms: 5000 } });
    roundtrip!(rt_media_cmd_vol,  Packet::MediaCommand { action: MediaAction::VolumeSet { pct: 75 } });

    roundtrip!(rt_input, Packet::InputEvent { kind: InputKind::MouseMove, data: serde_json::json!({"x":10,"y":20}) });
    roundtrip!(rt_run_req, Packet::RunCommandRequest { command_id: "c1".into() });
    roundtrip!(rt_run_out, Packet::RunCommandOutput { command_id: "c1".into(), stdout: "ok".into(), stderr: "".into(), exit_code: 0 });
    roundtrip!(rt_screen_start, Packet::ScreenShareStart);
    roundtrip!(rt_screen_stop,  Packet::ScreenShareStop);
    roundtrip!(rt_webcam_start, Packet::WebcamStart);
    roundtrip!(rt_webcam_stop,  Packet::WebcamStop);

    roundtrip!(rt_sdp_offer,  Packet::WebRtcSdpOffer { session_id: "s1".into(), sdp: "v=0".into() });
    roundtrip!(rt_sdp_answer, Packet::WebRtcSdpAnswer { session_id: "s1".into(), sdp: "v=0".into() });
    roundtrip!(rt_ice,        Packet::WebRtcIceCandidate { session_id: "s1".into(), candidate: "c".into(), sdp_mid: "0".into(), sdp_m_line_index: 0 });
    roundtrip!(rt_rtc_close,  Packet::WebRtcClose { session_id: "s1".into() });

    #[test]
    fn unknown_type_returns_error_not_panic() {
        let json = r#"{"type":"totally_unknown_variant_xyz"}"#;
        assert!(Packet::from_json(json).is_err(), "unknown type must error");
    }

    #[test]
    fn hello_missing_field_returns_error() {
        // `name` field is absent — serde must return Err, not a panic.
        let json = r#"{"type":"hello","device_id":"id1","version":1}"#;
        assert!(Packet::from_json(json).is_err(), "missing name must error");
    }
}
