//! sttts fork: remote (OpenAI-compatible) transcription provider.
//!
//! Instead of running local Whisper/Parakeet inference, the recorded audio is
//! POSTed to an OpenAI-compatible `/v1/audio/transcriptions` endpoint — e.g. the
//! DGX Spark gateway running Nemotron 3.5 ASR behind a model-proxy.
//!
//! The rest of Handy (hotkey, mic capture, VAD, paste, overlay, post-process)
//! is untouched; this module only replaces the inference step.

use log::{debug, info};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::Deserialize;
use std::io::Write;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RemoteTranscribeConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub language: Option<String>,
    /// Optional glossary/known-words prompt passed through if the server honors it.
    pub prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    #[serde(default)]
    text: String,
}

/// Encode f32 mono samples (any sample rate) as a 16-bit PCM WAV in-memory.
pub fn f32_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let mut wav: Vec<u8> = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits
    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = s.clamp(-1.0, 1.0);
        let q = if v < 0.0 { v * 32768.0 } else { v * 32767.0 } as i16;
        wav.extend_from_slice(&q.to_le_bytes());
    }
    wav
}

/// POST audio to an OpenAI-compatible transcriptions endpoint, return the text.
pub async fn remote_transcribe(
    cfg: &RemoteTranscribeConfig,
    wav: Vec<u8>,
) -> Result<String, String> {
    let url = format!(
        "{}/v1/audio/transcriptions",
        cfg.base_url.trim_end_matches('/')
    );

    let mut headers = HeaderMap::new();
    if !cfg.api_key.is_empty() {
        let val = HeaderValue::from_str(&format!("Bearer {}", cfg.api_key))
            .map_err(|e| format!("invalid api key header: {e}"))?;
        headers.insert(AUTHORIZATION, val);
    }

    let mut form = reqwest::multipart::Form::new()
        .text("model", cfg.model.clone())
        .part(
            "file",
            reqwest::multipart::Part::bytes(wav)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|e| e.to_string())?,
        );
    if let Some(lang) = &cfg.language {
        if lang != "auto" && !lang.is_empty() {
            form = form.text("language", lang.clone());
        }
    }
    if let Some(prompt) = &cfg.prompt {
        if !prompt.is_empty() {
            form = form.text("prompt", prompt.clone());
        }
    }

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let t0 = std::time::Instant::now();
    let resp = client.post(&url).multipart(form).send().await.map_err(|e| {
        format!("request to {url} failed: {e}")
    })?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("endpoint returned {status}: {}", body.chars().take(300).collect::<String>()));
    }

    let parsed: TranscriptionResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse response JSON: {e}"))?;

    info!(
        "sttts remote transcription ok in {:?} ({} chars)",
        t0.elapsed(),
        parsed.text.len()
    );
    debug!("sttts remote text: {:?}", parsed.text);
    Ok(parsed.text)
}

/// Sync wrapper used from the (sync) transcription manager call site.
pub fn remote_transcribe_blocking(
    cfg: &RemoteTranscribeConfig,
    samples: &[f32],
    sample_rate: u32,
) -> Result<String, String> {
    let wav = f32_to_wav(samples, sample_rate);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    rt.block_on(remote_transcribe(cfg, wav))
}
