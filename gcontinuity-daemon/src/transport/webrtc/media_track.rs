//! Media track stub for Phase 2.
#![allow(dead_code)] // Phase 5/6 will wire GStreamer / v4l2 here
//!
//! Phase 2: log the attached track and return.
//! Phase 5: pipe video/audio to GStreamer.
//! Phase 6: pipe camera to v4l2loopback.

use anyhow::Result;
use std::sync::Arc;
use webrtc::track::track_remote::TrackRemote;

/// Which kind of media this receiver is handling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaKind {
    ScreenShare,
    Camera,
}

/// Placeholder receiver for an incoming WebRTC media track.
pub struct MediaReceiver {
    /// The WebRTC session this track belongs to.
    pub session_id: String,
    /// What this track carries.
    pub kind: MediaKind,
}

impl MediaReceiver {
    /// Create a new receiver stub.
    pub fn new(session_id: &str, kind: MediaKind) -> Self {
        Self {
            session_id: session_id.to_string(),
            kind,
        }
    }

    /// Attach a remote track.  Phase 2 implementation logs the track codec
    /// and does nothing else; Phase 5/6 will wire this to GStreamer / v4l2.
    pub async fn attach_track(&self, track: Arc<TrackRemote>) -> Result<()> {
        tracing::info!(
            session_id = %self.session_id,
            kind = ?self.kind,
            codec = %track.codec().capability.mime_type,
            ssrc  = track.ssrc(),
            "Media track attached — Phase 2 stub, no consumer yet"
        );
        Ok(())
    }
}
