//! Transport integration tests — in-process mock Android WebSocket client.
//!
//! Each test spins up a real TLS WebSocket server on a random free port,
//! connects with a Tokio-based client, and drives the protocol.
#![allow(dead_code)] // Phase 1 modules compiled in are not all exercised here

use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use gcontinuity_daemon as daemon;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Pick a free TCP port for the test server.
fn free_port() -> u16 {
    portpicker::pick_unused_port().expect("No free ports")
}

/// Build a `Config` with a single paired device using the test TLS fingerprint.
async fn test_config(
    data_dir: &std::path::Path,
    port: u16,
    cert_sha256: [u8; 32],
) -> Arc<daemon::config::Config> {
    let paired = daemon::config::PairedDevice {
        device_id: "test-android-001".to_string(),
        name: "Test Phone".to_string(),
        cert_sha256_hex: hex::encode(cert_sha256),
    };
    Arc::new(daemon::config::Config {
        port,
        data_dir: data_dir.to_path_buf(),
        device_name: "Test Linux".to_string(),
        paired_devices: vec![paired],
        download_dir: data_dir.to_path_buf(),
    })
}

/// Start the transport server and return (cancel_token, event_rx, feature_rx).
async fn start_server(
    config: Arc<daemon::config::Config>,
    tls: Arc<daemon::tls::TlsIdentity>,
) -> (
    CancellationToken,
    broadcast::Receiver<daemon::transport::websocket_server::TransportEvent>,
    mpsc::Receiver<daemon::transport::FeatureEvent>,
) {
    let registry  = Arc::new(daemon::transport::PeerRegistry::new());
    let (event_tx, event_rx) = broadcast::channel(64);
    let (feat_tx, feat_rx)   = mpsc::channel(64);
    let cancel     = CancellationToken::new();
    let cancel_srv = cancel.clone();

    tokio::spawn(daemon::transport::run_server(
        config, tls, registry, event_tx, feat_tx, cancel_srv,
    ));

    // Give the server a tick to bind.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (cancel, event_rx, feat_rx)
}

/// Connect a TLS WebSocket client to the server (no cert verification — test only).
async fn connect_client(port: u16) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let url = format!("wss://127.0.0.1:{port}");
    let req = url.into_client_request().unwrap();

    let connector = tokio_tungstenite::Connector::NativeTls(
        native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap(),
    );

    let (ws, _) = tokio_tungstenite::connect_async_tls_with_config(
        req, None, false, Some(connector),
    )
    .await
    .expect("Client connect failed");

    ws
}

// ── Helper: send a JSON value as a Text WS frame ──────────────────────────────

fn json_msg(v: serde_json::Value) -> tokio_tungstenite::tungstenite::Message {
    tokio_tungstenite::tungstenite::Message::Text(v.to_string())
}

fn str_msg(s: &str) -> tokio_tungstenite::tungstenite::Message {
    tokio_tungstenite::tungstenite::Message::Text(s.to_string())
}

// ── Test 1: Happy-path hello/ack handshake ────────────────────────────────────

#[tokio::test]
async fn test_hello_ack_handshake() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir  = tempdir().unwrap();
    let port = free_port();
    let tls  = Arc::new(daemon::tls::load_or_generate(dir.path()).await.unwrap());
    let cfg  = test_config(dir.path(), port, tls.cert_sha256).await;

    let (cancel, mut event_rx, _) = start_server(cfg, tls).await;

    let mut ws = connect_client(port).await;

    use futures_util::SinkExt;
    ws.send(json_msg(serde_json::json!({
        "type": "hello",
        "device_id": "test-android-001",
        "name": "Test Phone",
        "version": 1
    }))).await.unwrap();

    use futures_util::StreamExt;
    let ack_msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await.expect("Ack timeout").unwrap().unwrap();
    assert!(ack_msg.to_text().unwrap().contains("\"ack\""));

    let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
        .await.expect("Event timeout").unwrap();
    assert!(matches!(
        event,
        daemon::transport::websocket_server::TransportEvent::DeviceConnected { .. }
    ));

    cancel.cancel();
}

// ── Test 2: Reject unknown device ─────────────────────────────────────────────

#[tokio::test]
async fn test_rejects_unknown_device() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir  = tempdir().unwrap();
    let port = free_port();
    let tls  = Arc::new(daemon::tls::load_or_generate(dir.path()).await.unwrap());

    let cfg = Arc::new(daemon::config::Config {
        port,
        data_dir: dir.path().to_path_buf(),
        device_name: "Test Linux".to_string(),
        paired_devices: vec![],
        download_dir: dir.path().to_path_buf(),
    });

    let (cancel, _, _) = start_server(cfg, tls).await;
    let mut ws = connect_client(port).await;

    use futures_util::SinkExt;
    ws.send(json_msg(serde_json::json!({
        "type": "hello", "device_id": "unknown-device",
        "name": "Stranger", "version": 1
    }))).await.unwrap();

    use futures_util::StreamExt;
    let response = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await.expect("Response timeout");

    match response {
        Some(Ok(msg)) => {
            let text = msg.to_text().unwrap_or("");
            assert!(
                text.contains("reject") || text.contains("unknown"),
                "Expected reject message, got: {text}"
            );
        }
        Some(Err(_)) | None => {
            // Connection closed — also acceptable.
        }
    }

    cancel.cancel();
}

// ── Test 3: Ping/pong keepalive ───────────────────────────────────────────────

#[tokio::test]
async fn test_ping_pong() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir  = tempdir().unwrap();
    let port = free_port();
    let tls  = Arc::new(daemon::tls::load_or_generate(dir.path()).await.unwrap());
    let cfg  = test_config(dir.path(), port, tls.cert_sha256).await;
    let (cancel, _, _) = start_server(cfg, tls).await;

    let mut ws = connect_client(port).await;

    use futures_util::{SinkExt, StreamExt};

    ws.send(json_msg(serde_json::json!({
        "type": "hello", "device_id": "test-android-001",
        "name": "Test Phone", "version": 1
    }))).await.unwrap();
    let _ack = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap();

    ws.send(str_msg(r#"{"type":"ping"}"#)).await.unwrap();

    let pong = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await.expect("Pong timeout").unwrap().unwrap();
    assert!(pong.to_text().unwrap().contains("\"pong\""));

    cancel.cancel();
}

// ── Test 4: SessionResume ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_session_resume() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir  = tempdir().unwrap();
    let port = free_port();
    let tls  = Arc::new(daemon::tls::load_or_generate(dir.path()).await.unwrap());
    let cfg  = test_config(dir.path(), port, tls.cert_sha256).await;
    let (cancel, _ev, _) = start_server(cfg, tls).await;

    use futures_util::{SinkExt, StreamExt};

    let mut ws = connect_client(port).await;
    ws.send(str_msg(
        r#"{"type":"session_resume","session_token":"00000000-0000-0000-0000-000000000000"}"#
    )).await.unwrap();

    // Either Ack or close — no panic either way.
    let _ = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;

    cancel.cancel();
}

// ── Test 5: Broadcast to multiple peers ───────────────────────────────────────

#[tokio::test]
async fn test_broadcast_multiple_peers() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir  = tempdir().unwrap();
    let port = free_port();
    let tls  = Arc::new(daemon::tls::load_or_generate(dir.path()).await.unwrap());

    let cfg = Arc::new(daemon::config::Config {
        port,
        data_dir: dir.path().to_path_buf(),
        device_name: "Test Linux".to_string(),
        paired_devices: vec![
            daemon::config::PairedDevice {
                device_id: "dev-a".to_string(), name: "Phone A".to_string(),
                cert_sha256_hex: hex::encode(tls.cert_sha256),
            },
            daemon::config::PairedDevice {
                device_id: "dev-b".to_string(), name: "Phone B".to_string(),
                cert_sha256_hex: hex::encode(tls.cert_sha256),
            },
        ],
        download_dir: dir.path().to_path_buf(),
    });

    let (cancel, mut ev, _) = start_server(cfg, tls).await;

    use futures_util::{SinkExt, StreamExt};

    let mut ws_a = connect_client(port).await;
    let mut ws_b = connect_client(port).await;

    for (ws, id, name) in [(&mut ws_a, "dev-a", "Phone A"), (&mut ws_b, "dev-b", "Phone B")] {
        ws.send(json_msg(serde_json::json!({
            "type": "hello", "device_id": id, "name": name, "version": 1
        }))).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), ws.next()).await; // Ack
    }

    let mut connected = 0u32;
    for _ in 0..2 {
        if let Ok(Ok(daemon::transport::websocket_server::TransportEvent::DeviceConnected { .. })) =
            tokio::time::timeout(Duration::from_secs(5), ev.recv()).await
        {
            connected += 1;
        }
    }
    assert_eq!(connected, 2, "Both peers must fire DeviceConnected");

    cancel.cancel();
}

// ── Test 6: 100 MB file transfer via FileTransferReceiver (in-process) ─────────

#[tokio::test]
async fn test_file_transfer_100mb() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    use daemon::transport::webrtc::data_channel::FileTransferReceiver;
    use sha2::{Digest, Sha256};
    use webrtc::data_channel::data_channel_message::DataChannelMessage;

    let dir = tempdir().unwrap();
    let (ev_tx, _) = broadcast::channel(8);
    let mut receiver = FileTransferReceiver::new("test-file-001", dir.path(), ev_tx);

    const SIZE: usize = 100 * 1024 * 1024;
    let data: Vec<u8> = (0..SIZE).map(|i| (i % 251) as u8).collect();
    let expected_sha256 = hex::encode(Sha256::digest(&data));

    // Header frame.
    let total_chunks = data.len().div_ceil(65_536) as u32;
    receiver.on_message(DataChannelMessage {
        is_string: true,
        data: bytes::Bytes::from(serde_json::json!({
            "file_id": "test-file-001",
            "name": "bigfile.bin",
            "size": SIZE,
            "total_chunks": total_chunks
        }).to_string()),
    }).await.unwrap();

    // Chunk frames.
    for (idx, chunk) in data.chunks(65_536).enumerate() {
        let mut frame = Vec::with_capacity(4 + chunk.len());
        frame.extend_from_slice(&(idx as u32).to_le_bytes());
        frame.extend_from_slice(chunk);
        receiver.on_message(DataChannelMessage {
            is_string: false,
            data: bytes::Bytes::from(frame),
        }).await.unwrap();
    }

    // EOF frame.
    let result = receiver.on_message(DataChannelMessage {
        is_string: true,
        data: bytes::Bytes::from(serde_json::json!({
            "type": "eof",
            "file_id": "test-file-001",
            "sha256": expected_sha256
        }).to_string()),
    }).await.unwrap();

    assert!(result.is_some(), "100 MB transfer must complete");
    let dest = result.unwrap();
    let written = std::fs::read(&dest).unwrap();
    assert_eq!(written.len(), SIZE, "Written file size must match");
}

// ── Test 7: Graceful shutdown sends Disconnect ────────────────────────────────

#[tokio::test]
async fn test_graceful_shutdown() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir  = tempdir().unwrap();
    let port = free_port();
    let tls  = Arc::new(daemon::tls::load_or_generate(dir.path()).await.unwrap());
    let cfg  = test_config(dir.path(), port, tls.cert_sha256).await;
    let (cancel, _, _) = start_server(cfg, tls).await;

    use futures_util::{SinkExt, StreamExt};
    let mut ws = connect_client(port).await;

    ws.send(json_msg(serde_json::json!({
        "type": "hello", "device_id": "test-android-001",
        "name": "Test Phone", "version": 1
    }))).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), ws.next()).await; // Ack

    cancel.cancel();

    let mut got_disconnect = false;
    while let Ok(Some(Ok(msg))) =
        tokio::time::timeout(Duration::from_secs(5), ws.next()).await
    {
        if let Ok(text) = msg.to_text() {
            if text.contains("\"disconnect\"") {
                got_disconnect = true;
                break;
            }
        }
    }
    assert!(got_disconnect, "Server must send Disconnect on shutdown");
}
