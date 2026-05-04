//! WebRTC peer connection management.
#![allow(dead_code)] // Phase 3+ will use all public APIs

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use webrtc::api::APIBuilder;
use webrtc::api::API;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

/// Lifecycle of a WebRTC session.
#[derive(Debug, Clone, PartialEq)]
pub enum WebRtcState {
    /// Created but signaling not started.
    Idle,
    /// SDP offer/answer exchange in progress.
    Signaling,
    /// ICE candidates exchanging; DTLS handshake pending.
    Connecting,
    /// Data channels and/or media tracks are live.
    Active,
    /// Graceful teardown in progress.
    Closing,
    /// Connection failed; contains human-readable reason.
    Failed(String),
}

/// Whether this side initiated the session (sent the offer) or accepted it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionRole {
    Offerer,
    Answerer,
}

/// One WebRTC session, identified by a UUID assigned by the initiating side.
pub struct WebRtcSession {
    /// UUID shared with the remote peer in signaling packets.
    pub session_id: String,
    /// The Android peer that owns this session.
    pub device_id: String,
    /// The live libwebrtc peer connection.
    pub peer_connection: Arc<RTCPeerConnection>,
    /// Mutable lifecycle state — locked only for state transitions.
    pub state: Arc<Mutex<WebRtcState>>,
}

/// Manages all WebRTC sessions for this daemon instance.
///
/// `RwLock` is used because session lookup (read) is far more common than
/// creation/teardown (write).
pub struct WebRtcManager {
    sessions: Arc<RwLock<HashMap<String, WebRtcSession>>>,
    api: API,
}

impl WebRtcManager {
    /// Create a new manager.  The `webrtc::api::API` is built once; creating
    /// it is cheap but not `Clone`, so we wrap it here.
    pub fn new() -> Self {
        // LAN-only: empty ICE server list avoids any STUN/TURN traffic.
        let api = APIBuilder::new().build();
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            api,
        }
    }

    /// Create a new `RTCPeerConnection` for the given session.  Returns the
    /// Arc so the caller can attach data channels / tracks before signaling.
    pub async fn create_session(
        &self,
        session_id: &str,
        device_id: &str,
        _role: SessionRole,
    ) -> Result<Arc<RTCPeerConnection>> {
        let config = RTCConfiguration {
            // Intentionally empty — LAN-only, no STUN/TURN.
            ice_servers: vec![],
            ..Default::default()
        };

        let pc = Arc::new(
            self.api
                .new_peer_connection(config)
                .await
                .context("Failed to create RTCPeerConnection")?,
        );

        let session = WebRtcSession {
            session_id: session_id.to_string(),
            device_id: device_id.to_string(),
            peer_connection: pc.clone(),
            state: Arc::new(Mutex::new(WebRtcState::Signaling)),
        };

        self.sessions
            .write()
            .await
            .insert(session_id.to_string(), session);

        tracing::info!(
            session_id = %session_id,
            device_id = %device_id,
            "WebRTC session created"
        );

        Ok(pc)
    }

    /// Apply a remote SDP answer received via the signaling channel.
    pub async fn handle_sdp_answer(&self, session_id: &str, sdp: &str) -> Result<()> {
        let guard = self.sessions.read().await;
        let session = guard
            .get(session_id)
            .with_context(|| format!("Unknown WebRTC session '{session_id}'"))?;

        let answer = RTCSessionDescription::answer(sdp.to_string())
            .context("Failed to parse SDP answer")?;

        session
            .peer_connection
            .set_remote_description(answer)
            .await
            .context("set_remote_description failed")?;

        *session.state.lock().await = WebRtcState::Connecting;
        tracing::debug!(session_id = %session_id, "Applied SDP answer");
        Ok(())
    }

    /// Add a remote ICE candidate received via the signaling channel.
    pub async fn handle_ice_candidate(
        &self,
        session_id: &str,
        candidate: &str,
        mid: &str,
        idx: u32,
    ) -> Result<()> {
        let guard = self.sessions.read().await;
        let session = guard
            .get(session_id)
            .with_context(|| format!("Unknown WebRTC session '{session_id}'"))?;

        let init = RTCIceCandidateInit {
            candidate: candidate.to_string(),
            sdp_mid: Some(mid.to_string()),
            sdp_mline_index: Some(idx as u16),
            username_fragment: None,
        };

        session
            .peer_connection
            .add_ice_candidate(init)
            .await
            .context("add_ice_candidate failed")?;

        tracing::debug!(session_id = %session_id, "Added ICE candidate");
        Ok(())
    }

    /// Close a session and remove it from the registry.
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        let session = {
            let mut guard = self.sessions.write().await;
            guard
                .remove(session_id)
                .with_context(|| format!("Unknown WebRTC session '{session_id}'"))?
        };

        *session.state.lock().await = WebRtcState::Closing;
        session
            .peer_connection
            .close()
            .await
            .context("RTCPeerConnection::close failed")?;

        tracing::info!(session_id = %session_id, "WebRTC session closed");
        Ok(())
    }

    /// Return all active session IDs (used for status reporting).
    pub async fn active_sessions(&self) -> Vec<String> {
        self.sessions.read().await.keys().cloned().collect()
    }
}

impl Default for WebRtcManager {
    fn default() -> Self {
        Self::new()
    }
}
