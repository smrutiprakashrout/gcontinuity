use futures_util::SinkExt;
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use futures_util::stream::SplitSink;
use tokio_tungstenite::WebSocketStream;
use tokio_rustls::server::TlsStream;
use tokio::net::TcpStream;
use gcontinuity_common::Packet;

pub struct KeepaliveTask {
    pub ws_tx: Arc<Mutex<SplitSink<WebSocketStream<TlsStream<TcpStream>>, Message>>>,
    pub last_pong: Arc<Mutex<Instant>>,
    pub disconnect_tx: oneshot::Sender<()>,
}

impl KeepaliveTask {
    pub fn spawn(
        ws_tx: Arc<Mutex<SplitSink<WebSocketStream<TlsStream<TcpStream>>, Message>>>,
        last_pong: Arc<Mutex<Instant>>,
        disconnect_tx: oneshot::Sender<()>,
    ) -> JoinHandle<()> {
        let task = Self {
            ws_tx,
            last_pong,
            disconnect_tx,
        };
        tokio::spawn(task.run())
    }

    pub async fn run(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        let mut consecutive_misses = 0;

        loop {
            interval.tick().await;

            let timestamp_ms = now_ms();
            let ping_packet = Packet::Ping { timestamp_ms };
            
            if let Ok(json) = ping_packet.to_json() {
                let mut sink = self.ws_tx.lock().await;
                if sink.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }

            // Wait 10 seconds for pong
            tokio::time::sleep(Duration::from_secs(10)).await;

            let last_pong_val = *self.last_pong.lock().await;
            let now = Instant::now();
            
            if now.duration_since(last_pong_val) > Duration::from_secs(35) {
                consecutive_misses += 1;
            } else {
                consecutive_misses = 0;
            }

            if consecutive_misses >= 2 {
                tracing::warn!("Connection dead — 2 missed PINGs");
                let _ = self.disconnect_tx.send(());
                break;
            }
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
