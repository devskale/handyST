//! sttts fork: remote streaming transcription client.
//!
//! Streams live 16 kHz mono PCM to an OpenAI-compatible gateway over a
//! WebSocket (`/v1/audio/transcriptions/stream`) and surfaces the partial
//! transcripts as they arrive. Protocol (mirrored by the web demo in
//! sttts/web):
//!
//! * client sends raw int16 little-endian PCM as binary frames
//! * an EMPTY binary frame signals end-of-audio; the client then keeps the
//!   connection open to receive the final
//! * server sends `{"type":"partial","partial":true,"text":...}` while
//!   streaming and `{"type":"final","partial":false,"text":...}` after the
//!   end-of-audio frame
//!
//! If the socket dies mid-stream the caller still holds the recorded samples
//! and can fall back to the batch endpoint (see `remote_transcribe.rs`).

use crate::remote_transcribe::RemoteTranscribeConfig;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// How long to wait for the final after the end-of-audio frame before
/// giving up (the caller then falls back to batch transcription).
const FINAL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the WebSocket handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Deserialize)]
struct StreamMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
    /// The final is flagged `partial:false`; tolerate servers that only send
    /// the boolean without a type field.
    #[serde(default = "default_true")]
    partial: bool,
}

fn default_true() -> bool {
    true
}

fn is_final_message(msg: &StreamMessage) -> bool {
    msg.kind == "final" || !msg.partial
}

/// Derive the WebSocket URL from the HTTP base URL:
/// `http://dgxp:3001` → `ws://dgxp:3001/v1/audio/transcriptions/stream`.
pub fn stream_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws_base = match base.strip_prefix("https://") {
        Some(rest) => format!("wss://{rest}"),
        None => match base.strip_prefix("http://") {
            Some(rest) => format!("ws://{rest}"),
            None => format!("ws://{base}"),
        },
    };
    format!("{ws_base}/v1/audio/transcriptions/stream")
}

/// Convert an f32 mono frame to raw little-endian int16 bytes.
pub fn f32_to_pcm16(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        let v = s.clamp(-1.0, 1.0);
        let q = if v < 0.0 { v * 32768.0 } else { v * 32767.0 } as i16;
        bytes.extend_from_slice(&q.to_le_bytes());
    }
    bytes
}

/// Live handle to a streaming transcription session. Cheap to clone into the
/// audio recorder's per-frame callback; the session ends via
/// [`RemoteStream::finalize`] / [`RemoteStream::abort`].
pub struct RemoteStream {
    feed_tx: mpsc::Sender<StreamFrame>,
    result_rx: Mutex<Option<mpsc::Receiver<Result<String, String>>>>,
    /// Closed as soon as the socket task ends (error or final delivered).
    closed: Arc<AtomicBool>,
    /// True until the session is finalized or aborted.
    active: Arc<AtomicBool>,
}

enum StreamFrame {
    Pcm(Vec<u8>),
    EndOfAudio,
}

impl RemoteStream {
    /// Open a streaming session. Returns once the WebSocket handshake
    /// succeeded, so callers know feeds will actually be delivered.
    pub async fn connect(
        cfg: &RemoteTranscribeConfig,
        on_partial: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let url = stream_url(&cfg.base_url);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| format!("invalid stream URL {url}: {e}"))?;
        if !cfg.api_key.is_empty() {
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {}", cfg.api_key)
                    .parse()
                    .map_err(|e| format!("invalid api key header: {e}"))?,
            );
        }

        let (ws, _resp) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| format!("connect to {url} timed out"))?
            .map_err(|e| format!("connect to {url} failed: {e}"))?;

        let (feed_tx, feed_rx) = mpsc::channel::<StreamFrame>();
        let (result_tx, result_rx) = mpsc::channel::<Result<String, String>>();
        let closed = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(true));

        // One task per direction. The writer drains the feed channel (binary
        // PCM frames, then the empty end-of-audio frame); the reader parses
        // server JSON, invokes the partial callback, and delivers the final.
        let (mut writer, mut reader) = ws.split();
        let closed_for_writer = Arc::clone(&closed);
        tokio::spawn(async move {
            while let Ok(frame) = feed_rx.recv() {
                let is_end_of_audio = matches!(frame, StreamFrame::EndOfAudio);
                let message = match frame {
                    StreamFrame::Pcm(bytes) => Message::Binary(bytes.into()),
                    StreamFrame::EndOfAudio => Message::Binary(Vec::new().into()),
                };
                if writer.send(message).await.is_err() {
                    closed_for_writer.store(true, Ordering::Relaxed);
                    return;
                }
                if is_end_of_audio {
                    // socket half-close is left to the server; the reader
                    // delivers the final
                    return;
                }
            }
        });

        let closed_for_reader = Arc::clone(&closed);
        tokio::spawn(async move {
            let outcome = loop {
                match reader.next().await {
                    Some(Ok(Message::Text(json))) => {
                        match serde_json::from_str::<StreamMessage>(&json) {
                            Ok(msg) if is_final_message(&msg) => break Ok(msg.text),
                            Ok(msg) if !msg.text.is_empty() => on_partial(msg.text),
                            Ok(_) => {}
                            Err(e) => {
                                log::debug!("sttts stream: unparsable message ({e}): {json}");
                            }
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        break Err(format!("stream closed by server: {frame:?}"));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Err(format!("stream error: {e}")),
                    None => {
                        break Err("stream ended without a final".to_string());
                    }
                }
            };
            let _ = result_tx.send(outcome);
            closed_for_reader.store(true, Ordering::Relaxed);
        });

        Ok(Self {
            feed_tx,
            result_rx: Mutex::new(Some(result_rx)),
            closed,
            active,
        })
    }

    /// Forward a 16 kHz mono frame to the stream.
    pub fn feed(&self, samples: &[f32]) {
        if !self.active.load(Ordering::Relaxed) {
            return;
        }
        let _ = self.feed_tx.send(StreamFrame::Pcm(f32_to_pcm16(samples)));
    }

    /// Signal end-of-audio and block until the final arrives (or the
    /// [`FINAL_TIMEOUT`] expires). Must be called off the audio thread.
    pub fn finalize(&self) -> Result<String, String> {
        if !self.active.swap(false, Ordering::Relaxed) {
            return Err("stream already finalized or aborted".to_string());
        }
        let _ = self.feed_tx.send(StreamFrame::EndOfAudio);
        let rx = self
            .result_rx
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| "stream already finalized".to_string())?;
        let deadline = Instant::now() + FINAL_TIMEOUT;
        match rx.recv_timeout(deadline - Instant::now()) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(format!("no final within {:?}", FINAL_TIMEOUT))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("stream task exited without a final".to_string())
            }
        }
    }

    /// Tear the session down without waiting for a final (cancel path).
    pub fn abort(&self) {
        self.active.store(false, Ordering::Relaxed);
        // Dropping the feed sender ends the writer task; the reader task ends
        // with the socket once the writer side closes it. If the socket is
        // somehow wedged, `closed` still reports the session as gone.
        self.result_rx.lock().unwrap().take();
    }

    /// Whether the underlying socket is still alive.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}
