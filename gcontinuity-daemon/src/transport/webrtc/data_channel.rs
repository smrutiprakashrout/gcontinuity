//! File transfer over a WebRTC data channel.
#![allow(dead_code)] // Phase 3+ will activate FileTransfer types
//!
//! Frame protocol:
//!   HEADER → JSON text frame: `{"file_id":..,"name":..,"size":..,"total_chunks":..}`
//!   CHUNKS → binary frames:   [4-byte chunk_index LE][payload up to CHUNK_SIZE bytes]
//!   EOF    → JSON text frame: `{"type":"eof","file_id":..,"sha256":<hex>}`
//!
//! SHA-256 of the assembled file must match the EOF checksum; on mismatch
//! the temp file is deleted and an error is returned.

use anyhow::{Context, Result};
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::broadcast;
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;

/// Payload size per binary chunk.  64 KB balances throughput and memory.
const CHUNK_SIZE: usize = 65_536;

/// Progress is emitted every 1 MB of data sent/received.
const PROGRESS_INTERVAL_BYTES: u64 = 1_048_576;

// ── Header / EOF frame shapes ─────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct FileHeader {
    file_id: String,
    name: String,
    size: u64,
    total_chunks: u32,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FileEof {
    #[serde(rename = "type")]
    kind: String, // always "eof"
    file_id: String,
    sha256: String,
}

// ── Sender ────────────────────────────────────────────────────────────────────

/// Sends a local file over a WebRTC data channel in 64 KB binary chunks.
pub struct FileTransferSender {
    /// Opaque identifier shared with the receiver.
    pub file_id: String,
    channel: Arc<RTCDataChannel>,
    event_tx: broadcast::Sender<crate::transport::websocket_server::TransportEvent>,
}

impl FileTransferSender {
    /// Create a sender.  The caller is responsible for negotiating the data
    /// channel and sending `FileSendOffer` before calling `send_file`.
    pub fn new(
        file_id: String,
        channel: Arc<RTCDataChannel>,
        event_tx: broadcast::Sender<crate::transport::websocket_server::TransportEvent>,
    ) -> Self {
        Self { file_id, channel, event_tx }
    }

    /// Open `path`, stream it over the data channel, then send an EOF frame
    /// containing the SHA-256 checksum of the full file.
    pub async fn send_file(&self, path: &Path) -> Result<()> {
        let metadata = tokio::fs::metadata(path)
            .await
            .context("Failed to stat file")?;
        let file_size = metadata.len();
        let total_chunks = file_size.div_ceil(CHUNK_SIZE as u64) as u32;

        // ── HEADER ───────────────────────────────────────────────────────────
        let file_name = path
            .file_name()
            .context("Path has no filename")?
            .to_string_lossy()
            .into_owned();

        let header = FileHeader {
            file_id: self.file_id.clone(),
            name: file_name,
            size: file_size,
            total_chunks,
        };
        let header_json = serde_json::to_string(&header).context("Serialise header")?;
        self.channel
            .send_text(header_json)
            .await
            .context("Send header frame")?;

        tracing::info!(
            file_id = %self.file_id,
            size = file_size,
            chunks = total_chunks,
            "File transfer started"
        );

        // ── CHUNKS ───────────────────────────────────────────────────────────
        let mut file = tokio::fs::File::open(path)
            .await
            .context("Open file for reading")?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut chunk_index: u32 = 0;
        let mut bytes_sent: u64 = 0;
        let mut last_progress: u64 = 0;

        loop {
            let n = file.read(&mut buf).await.context("Read chunk")?;
            if n == 0 {
                break;
            }

            let payload = &buf[..n];
            hasher.update(payload);

            // Binary frame: [4-byte LE index][payload]
            let mut frame = Vec::with_capacity(4 + n);
            frame.extend_from_slice(&chunk_index.to_le_bytes());
            frame.extend_from_slice(payload);

            self.channel
                .send(&Bytes::from(frame))
                .await
                .context("Send chunk frame")?;

            bytes_sent += n as u64;
            chunk_index += 1;

            // Emit progress every PROGRESS_INTERVAL_BYTES.
            if bytes_sent - last_progress >= PROGRESS_INTERVAL_BYTES {
                last_progress = bytes_sent;
                let _ = self.event_tx.send(
                    crate::transport::websocket_server::TransportEvent::FileProgress {
                        file_id: self.file_id.clone(),
                        bytes_done: bytes_sent,
                        total: file_size,
                    },
                );
            }
        }

        // ── EOF ──────────────────────────────────────────────────────────────
        let sha256_hex = hex::encode(hasher.finalize());
        let eof = FileEof {
            kind: "eof".into(),
            file_id: self.file_id.clone(),
            sha256: sha256_hex,
        };
        self.channel
            .send_text(serde_json::to_string(&eof).context("Serialise EOF")?)
            .await
            .context("Send EOF frame")?;

        tracing::info!(
            file_id = %self.file_id,
            bytes_sent,
            "File transfer complete"
        );
        Ok(())
    }
}

// ── Receiver ─────────────────────────────────────────────────────────────────

/// Receives file chunks from a WebRTC data channel, reassembles them, and
/// verifies the SHA-256 checksum before moving the file to `dest_dir`.
pub struct FileTransferReceiver {
    /// Opaque identifier shared with the sender.
    pub file_id: String,
    tmp_path: PathBuf,
    dest_dir: PathBuf,
    chunks: BTreeMap<u32, Vec<u8>>,
    total_chunks: Option<u32>,
    expected_sha256: Option<String>,
    bytes_received: u64,
    last_progress: u64,
    event_tx: broadcast::Sender<crate::transport::websocket_server::TransportEvent>,
}

impl FileTransferReceiver {
    /// Create a receiver.  Temp files live under `/tmp/gcontinuity/{file_id}`.
    pub fn new(
        file_id: &str,
        dest_dir: &Path,
        event_tx: broadcast::Sender<crate::transport::websocket_server::TransportEvent>,
    ) -> Self {
        let tmp_path = PathBuf::from("/tmp/gcontinuity").join(file_id);
        Self {
            file_id: file_id.to_string(),
            tmp_path,
            dest_dir: dest_dir.to_path_buf(),
            chunks: BTreeMap::new(),
            total_chunks: None,
            expected_sha256: None,
            bytes_received: 0,
            last_progress: 0,
            event_tx,
        }
    }

    /// Feed one message from the data channel.
    /// Returns `Some(final_path)` when the file is complete and verified,
    /// `None` for all intermediate frames.
    pub async fn on_message(&mut self, msg: DataChannelMessage) -> Result<Option<PathBuf>> {
        if msg.is_string {
            // Either HEADER or EOF text frame.
            let text = String::from_utf8(msg.data.to_vec())
                .context("Data channel text frame is not UTF-8")?;

            // Try EOF first (has a "type" field).
            if let Ok(eof) = serde_json::from_str::<FileEof>(&text) {
                if eof.kind == "eof" {
                    self.expected_sha256 = Some(eof.sha256);
                    return self.assemble().await.map(Some);
                }
            }

            // Otherwise it's the header.
            if let Ok(hdr) = serde_json::from_str::<FileHeader>(&text) {
                self.total_chunks = Some(hdr.total_chunks);
                tracing::info!(
                    file_id = %self.file_id,
                    name = %hdr.name,
                    size = hdr.size,
                    "File transfer incoming"
                );
            }
        } else {
            // Binary chunk: [4-byte LE index][payload]
            let data = msg.data;
            anyhow::ensure!(data.len() >= 4, "Chunk too short");
            let index = u32::from_le_bytes(data[..4].try_into().unwrap());
            let payload = data[4..].to_vec();
            self.bytes_received += payload.len() as u64;
            self.chunks.insert(index, payload);

            if self.bytes_received - self.last_progress >= PROGRESS_INTERVAL_BYTES {
                self.last_progress = self.bytes_received;
                let _ = self.event_tx.send(
                    crate::transport::websocket_server::TransportEvent::FileProgress {
                        file_id: self.file_id.clone(),
                        bytes_done: self.bytes_received,
                        total: 0, // total unknown until header is parsed
                    },
                );
            }
        }
        Ok(None)
    }

    /// Assemble all received chunks, verify SHA-256, and move to dest_dir.
    async fn assemble(&self) -> Result<PathBuf> {
        // Ensure temp directory exists.
        tokio::fs::create_dir_all("/tmp/gcontinuity")
            .await
            .context("Create /tmp/gcontinuity")?;

        // Write chunks in order to the temp path.
        let mut hasher = Sha256::new();
        let mut assembled: Vec<u8> = Vec::with_capacity(self.bytes_received as usize);
        for payload in self.chunks.values() {
            hasher.update(payload);
            assembled.extend_from_slice(payload);
        }

        let actual_sha256 = hex::encode(hasher.finalize());
        let expected = self
            .expected_sha256
            .as_deref()
            .context("EOF frame not yet received")?;

        if actual_sha256 != expected {
            // Clean up temp data and surface an error.
            let _ = tokio::fs::remove_file(&self.tmp_path).await;
            anyhow::bail!(
                "SHA-256 mismatch for file '{}': expected {expected}, got {actual_sha256}",
                self.file_id
            );
        }

        tokio::fs::create_dir_all(&self.dest_dir)
            .await
            .context("Create dest_dir")?;

        // Use file_id as temp name; a real implementation would use the header name.
        let dest_path = self.dest_dir.join(&self.file_id);
        tokio::fs::write(&dest_path, &assembled)
            .await
            .context("Write assembled file")?;

        tracing::info!(
            file_id = %self.file_id,
            dest = %dest_path.display(),
            "File transfer verified and saved"
        );
        Ok(dest_path)
    }
}


