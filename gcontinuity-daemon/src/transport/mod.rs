//! Transport-layer packet router.
//!
//! `route_packet` is the single dispatch point for every packet that arrives
//! from a peer.  Phase 2 handles handshake/WebRTC packets directly; all
//! Phase 3–6 packets are forwarded to `feature_tx` for later subsystems.

pub mod packet;
pub mod peer;
pub mod websocket_server;
pub mod webrtc;

pub use websocket_server::run_server;
pub use peer::PeerRegistry;
pub use webrtc::WebRtcManager;

use tokio::sync::{broadcast, mpsc};

use crate::transport::packet::Packet;
use crate::transport::peer::PeerRegistry as _PR;
use crate::transport::websocket_server::TransportEvent as _TE;

// ── Feature forwarding ────────────────────────────────────────────────────────

/// A Phase 3–6 packet that could not be handled in the transport layer.
/// Downstream subsystems subscribe to the feature channel and match on `packet`.
#[allow(dead_code)] // Phase 3+ consumers will read these fields
pub struct FeatureEvent {
    /// Which device sent the packet.
    pub device_id: String,
    /// The packet that needs feature-layer processing.
    pub packet: Packet,
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Dispatch an incoming packet to the correct subsystem.
///
/// * Handshake control (Ping/Pong) is handled inline.
/// * WebRTC signaling is forwarded to the `WebRtcManager`.
/// * All other packets are pushed to `feature_tx` for Phase 3–6 handlers.
pub async fn route_packet(
    packet: Packet,
    device_id: &str,
    registry: &_PR,
    feature_tx: &mpsc::Sender<FeatureEvent>,
    _event_tx: &broadcast::Sender<_TE>,
) {
    match packet {
        // ── Keepalive ─────────────────────────────────────────────────────────
        Packet::Ping => {
            // Respond immediately; no lock contention.
            registry.send_to(device_id, Packet::Pong).await.ok();
        }
        Packet::Pong => {
            // Keepalive timer reset is handled in the per-peer loop; nothing to
            // do at the router level.
        }

        // ── WebRTC signaling — no WebRtcManager reference here to keep the
        //    function signature simple; forward to feature_tx so the D-Bus
        //    bridge or a dedicated WebRTC task can pick it up. ────────────────
        pkt @ Packet::WebRtcSdpOffer    { .. }
        | pkt @ Packet::WebRtcSdpAnswer  { .. }
        | pkt @ Packet::WebRtcIceCandidate { .. }
        | pkt @ Packet::WebRtcClose      { .. } => {
            feature_tx
                .send(FeatureEvent { device_id: device_id.to_string(), packet: pkt })
                .await
                .ok();
        }

        // ── Phase 3–6 packets → feature layer ────────────────────────────────
        other => {
            feature_tx
                .send(FeatureEvent {
                    device_id: device_id.to_string(),
                    packet: other,
                })
                .await
                .ok();
        }
    }
}
