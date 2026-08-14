//! sttts fork: transcription manager.
//!
//! Local ML inference (transcribe-cpp / transcribe-rs engines, model loading)
//! has been removed. The manager coordinates remote transcription instead:
//! live audio streams to the gateway's WebSocket endpoint while recording
//! (see [`crate::remote_stream`]); a batch fallback POST exists for the
//! headless path and for stream failures (see [`crate::remote_transcribe`]).
//! Both paths run the shared text post-processing (custom-word correction,
//! filler removal, normalization).

use crate::audio_toolkit::{
    apply_custom_words, detect_output_language, normalize_transcription_output,
    remove_filler_words, OutputLanguageEvidence,
};
use crate::remote_stream::RemoteStream;
use crate::remote_transcribe::{remote_transcribe_blocking, RemoteTranscribeConfig};
use crate::settings::{get_settings, AppSettings};
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::AppHandle;
use tauri_specta::Event as _;

/// Sample rate the audio pipeline captures and stores (16 kHz mono PCM).
pub const SAMPLE_RATE: u32 = 16_000;

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Live transcription snapshot emitted to the overlay during a streaming run.
/// The remote stream has no committed/tentative split — the latest partial
/// arrives as `tentative` with an empty `committed`.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct StreamTextEvent {
    pub committed: String,
    pub tentative: String,
}

/// Phase of the streaming overlay card. The frontend starts each session in
/// `Listening`; Rust emits the transition to `Working` (finalize/polish).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum StreamPhase {
    Listening,
    Working,
}

/// Semantic kind of "working" phase, used to localize the spinner label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum StreamWorkKind {
    Transcribing,
    Polishing,
}

/// Emitted to switch the streaming overlay to a working spinner.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct StreamPhaseEvent {
    pub phase: StreamPhase,
    /// Present only when `phase` is `Working`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<StreamWorkKind>,
}

#[derive(Clone)]
pub struct TranscriptionManager {
    app_handle: AppHandle,
    router: Arc<StreamRouter>,
}

/// Routes real-time audio frames to the active remote streaming session.
/// Shared between the [`TranscriptionManager`] (opens/closes the route) and
/// the audio recorder's per-frame callback (feeds frames). A frame with no
/// stream open costs a single relaxed atomic load — no mutex lock, no Tauri
/// state lookup.
pub struct StreamRouter {
    session: Mutex<Option<Arc<RemoteStream>>>,
    /// True from `start_stream` until finalize/cancel. Checked first in the
    /// audio callback so feeding stays cheap.
    open: AtomicBool,
}

impl StreamRouter {
    fn new() -> Self {
        Self {
            session: Mutex::new(None),
            open: AtomicBool::new(false),
        }
    }

    /// Forward a 16 kHz frame to the active remote stream. Cheap no-op when
    /// no stream is open.
    pub fn feed(&self, frame: &[f32]) {
        if !self.open.load(Ordering::Relaxed) {
            return;
        }
        if let Some(stream) = self.session.lock().unwrap().as_ref() {
            stream.feed(frame);
        }
    }

    /// Whether a stream is pending or active.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }
}

impl TranscriptionManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        Ok(Self {
            app_handle: app_handle.clone(),
            router: Arc::new(StreamRouter::new()),
        })
    }

    /// The router handed to the audio recorder so its per-frame callback can
    /// feed the live stream directly.
    pub fn stream_router(&self) -> Arc<StreamRouter> {
        Arc::clone(&self.router)
    }

    /// Open a remote streaming session for the next recording. Partial
    /// transcripts are forwarded to the streaming overlay as they arrive.
    pub fn start_stream(&self) -> Result<()> {
        if self.router.is_open() {
            warn!("sttts: start_stream while a stream is still open — replacing it");
            self.cancel_stream();
        }

        let settings = get_settings(&self.app_handle);
        if !settings.remote_transcription_enabled {
            return Err(anyhow::anyhow!(
                "sttts: remote transcription disabled — enable it in Settings → Remote Transcription"
            ));
        }
        let cfg = remote_config(&settings);

        let app_handle = self.app_handle.clone();
        let on_partial = move |text: String| {
            let _ = StreamTextEvent {
                committed: String::new(),
                tentative: text,
            }
            .emit(&app_handle);
        };

        let stream = tauri::async_runtime::block_on(RemoteStream::connect(&cfg, on_partial))
            .map_err(anyhow::Error::msg)?;

        *self.router.session.lock().unwrap() = Some(Arc::new(stream));
        self.router.open.store(true, Ordering::Release);
        info!(
            "sttts: remote stream started ({} → {})",
            cfg.base_url,
            crate::remote_stream::stream_url(&cfg.base_url)
        );
        Ok(())
    }

    /// Whether a streaming session is open (feeding or awaiting finalize).
    pub fn is_streaming(&self) -> bool {
        self.router.is_open()
    }

    /// Signal end-of-audio and wait for the final transcript. `Ok(None)` when
    /// no stream is open (caller falls back to batch); `Err` when a stream
    /// was open but failed to deliver a final.
    pub fn finalize_stream(&self) -> Result<Option<String>> {
        if !self.router.open.swap(false, Ordering::Release) {
            return Ok(None);
        }
        let stream = self
            .router
            .session
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow::anyhow!("stream marked open but no session present"))?;
        match stream.finalize() {
            Ok(text) => Ok(Some(text)),
            Err(e) => Err(anyhow::anyhow!("remote stream failed: {}", e)),
        }
    }

    /// Tear down the active stream without waiting for a final (cancel path).
    pub fn cancel_stream(&self) {
        self.router.open.store(false, Ordering::Release);
        if let Some(stream) = self.router.session.lock().unwrap().take() {
            stream.abort();
        }
    }

    /// Emit a working-phase event to the streaming overlay (spinner + label).
    pub fn emit_stream_working(&self, kind: StreamWorkKind) {
        let _ = StreamPhaseEvent {
            phase: StreamPhase::Working,
            kind: Some(kind),
        }
        .emit(&self.app_handle);
    }

    /// Transcribe recorded PCM via the remote OpenAI-compatible endpoint and
    /// apply the shared text post-processing.
    pub fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        #[cfg(debug_assertions)]
        if std::env::var("HANDY_FORCE_TRANSCRIPTION_FAILURE").is_ok() {
            return Err(anyhow::anyhow!(
                "Simulated transcription failure (HANDY_FORCE_TRANSCRIPTION_FAILURE)"
            ));
        }

        let st = Instant::now();
        let audio_len = audio.len();

        if audio.is_empty() {
            debug!("Empty audio vector");
            return Ok(String::new());
        }

        let settings = get_settings(&self.app_handle);

        if !settings.remote_transcription_enabled {
            return Err(anyhow::anyhow!(
                "sttts: remote transcription disabled — enable it in Settings → Remote Transcription"
            ));
        }

        let language_hint = language_hint(&settings);
        let cfg = remote_config(&settings);

        debug!(
            "sttts: remote transcription via {}/v1/audio/transcriptions (model '{}', language {:?}, prompt {})",
            cfg.base_url,
            cfg.model,
            cfg.language,
            cfg.prompt.is_some()
        );

        let result =
            remote_transcribe_blocking(&cfg, &audio, SAMPLE_RATE).map_err(anyhow::Error::msg);

        let text = match result {
            Ok(text) => text,
            Err(err) => {
                error!("Remote transcription failed: {}", err);
                return Err(err);
            }
        };

        let filtered_result = post_process_remote_text(text, &settings, language_hint.as_deref());

        let elapsed_secs = st.elapsed().as_secs_f64();
        let audio_secs = audio_len as f64 / SAMPLE_RATE as f64;
        info!(
            "Remote transcription completed in {:.2}s for {:.2}s of audio ({:.2}x real-time)",
            elapsed_secs,
            audio_secs,
            real_time_factor(audio_secs, elapsed_secs)
        );

        if filtered_result.is_empty() {
            info!("Transcription result is empty");
        } else {
            info!("Transcription result: {}", filtered_result);
        }

        Ok(filtered_result)
    }

    /// Post-process a remote (batch or streamed) transcript with the shared
    /// text pipeline: custom-word correction, language-gated filler removal,
    /// normalization. Fail-open: a panicking transform returns the raw text.
    pub fn post_process(&self, raw: String, applied_language_hint: Option<&str>) -> String {
        let settings = get_settings(&self.app_handle);
        post_process_remote_text(raw, &settings, applied_language_hint)
    }
}

/// Shared post-processing for every remote transcript (batch and stream).
fn post_process_remote_text(
    raw: String,
    settings: &AppSettings,
    applied_language_hint: Option<&str>,
) -> String {
    // The remote endpoint received the language hint, so it is valid
    // evidence for the filler-removal gating below.
    let output_language = resolve_output_language_evidence(settings, applied_language_hint);
    let model_languages: Vec<String> = Vec::new();

    // Custom words were not handed to the model as an initial prompt
    // (support varies by endpoint), so always run the fuzzy correction.
    post_process_transcription_text(raw, settings, false, &output_language, &model_languages)
}

/// The endpoint configuration shared by the batch and streaming paths.
fn remote_config(settings: &AppSettings) -> RemoteTranscribeConfig {
    let prompt = if settings.custom_words.is_empty() {
        None
    } else {
        Some(settings.custom_words.join(", "))
    };
    RemoteTranscribeConfig {
        base_url: settings.remote_transcription_base_url.clone(),
        api_key: settings.remote_transcription_api_key.clone(),
        model: settings.remote_transcription_model.clone(),
        language: language_hint(settings),
        prompt,
    }
}

fn real_time_factor(audio_secs: f64, compute_secs: f64) -> f64 {
    if compute_secs > 0.0 {
        audio_secs / compute_secs
    } else {
        0.0
    }
}

/// The language hint to pass to the remote endpoint: the persisted selection,
/// unless it is "auto" (endpoint-side detection) or empty.
pub fn language_hint(settings: &AppSettings) -> Option<String> {
    match settings.selected_language.as_str() {
        "" | "auto" => None,
        other => Some(other.to_string()),
    }
}

/// Resolve how confidently Handy knows the language of the text produced by a
/// transcription run. The UI language is deliberately not part of this
/// decision.
fn resolve_output_language_evidence(
    settings: &AppSettings,
    applied_language_hint: Option<&str>,
) -> OutputLanguageEvidence {
    if let Some(language) = applied_language_hint.filter(|lang| !lang.is_empty() && *lang != "auto")
    {
        if settings.selected_language != "auto"
            && base_language_code(&settings.selected_language) == base_language_code(language)
        {
            return OutputLanguageEvidence::UserSelected(language.to_string());
        }

        // The endpoint may have required a concrete fallback even though the
        // user's persisted language was auto or unsupported.
        return OutputLanguageEvidence::ModelConstrained(language.to_string());
    }

    OutputLanguageEvidence::Unknown
}

fn base_language_code(language: &str) -> &str {
    language.split(&['-', '_'][..]).next().unwrap_or(language)
}

fn post_process_transcription_text(
    raw: String,
    settings: &AppSettings,
    custom_words_already_prompted: bool,
    output_language: &OutputLanguageEvidence,
    supported_languages: &[String],
) -> String {
    fail_open_text_transform(raw, |raw| {
        let corrected = if !settings.custom_words.is_empty() && !custom_words_already_prompted {
            apply_custom_words(
                &raw,
                &settings.custom_words,
                settings.word_correction_threshold,
            )
        } else {
            raw
        };

        // Last-resort language evidence: confidence-gated detection from the
        // transcribed text itself. Only consulted when it can change the
        // outcome (built-in gated fillers).
        let output_language = match output_language {
            OutputLanguageEvidence::Unknown
                if settings.filler_word_removal_enabled
                    && settings.custom_filler_words.is_none() =>
            {
                match detect_output_language(&corrected, supported_languages) {
                    Some(language) => {
                        debug!("Text-based language detection resolved '{}'", language);
                        OutputLanguageEvidence::TextDetected(language)
                    }
                    None => OutputLanguageEvidence::Unknown,
                }
            }
            other => other.clone(),
        };

        let without_fillers = remove_filler_words(
            &corrected,
            &output_language,
            &settings.custom_filler_words,
            settings.filler_word_removal_enabled,
        );

        normalize_transcription_output(&without_fillers)
    })
}

/// Optional text cleanup must never discard a successful result. The transform
/// is pure and owns its input, so recovering the untouched text is safe even if
/// a bug in custom-word or filler filtering unwinds.
fn fail_open_text_transform<F>(raw: String, transform: F) -> String
where
    F: FnOnce(String) -> String,
{
    let fallback = raw.clone();
    match catch_unwind(AssertUnwindSafe(|| transform(raw))) {
        Ok(processed) => processed,
        Err(payload) => {
            error!(
                "Optional transcription text post-processing panicked: {}; using the raw transcription",
                panic_payload_message(payload.as_ref())
            );
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn languages(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_string()).collect()
    }

    #[test]
    fn optional_text_transform_falls_back_to_raw_text_after_panic() {
        let raw = "原始轉錄。".to_string();
        let result = fail_open_text_transform(raw.clone(), |_| {
            panic!("simulated optional cleanup failure")
        });

        assert_eq!(result, raw);
    }

    #[test]
    fn portuguese_transcription_does_not_use_english_ui_filler_words() {
        let settings = AppSettings {
            app_language: "en".to_string(),
            selected_language: "pt-BR".to_string(),
            ..Default::default()
        };
        let supported = languages(&["en", "pt"]);
        let evidence = resolve_output_language_evidence(&settings, Some("pt"));

        let result = post_process_transcription_text(
            "eu vi um carro".to_string(),
            &settings,
            false,
            &evidence,
            &supported,
        );

        assert_eq!(
            evidence,
            OutputLanguageEvidence::UserSelected("pt".to_string())
        );
        assert_eq!(result, "eu vi um carro");
    }

    #[test]
    fn auto_language_without_detection_skips_gated_filler_removal() {
        let settings = AppSettings {
            selected_language: "auto".to_string(),
            ..Default::default()
        };
        let evidence =
            resolve_output_language_evidence(&settings, None);

        // Too short for a reliable text detection, so the gated "um" must
        // survive; the universal "uhm" is removed regardless.
        let result = post_process_transcription_text(
            "um uhm ok".to_string(),
            &settings,
            false,
            &evidence,
            &languages(&["en", "pt"]),
        );

        assert_eq!(evidence, OutputLanguageEvidence::Unknown);
        assert_eq!(result, "um ok");
    }

    #[test]
    fn unknown_evidence_with_confident_text_detection_removes_gated_fillers() {
        let settings = AppSettings {
            selected_language: "auto".to_string(),
            ..Default::default()
        };

        let result = post_process_transcription_text(
            "um so the weather forecast said it would probably rain throughout the whole weekend"
                .to_string(),
            &settings,
            false,
            &OutputLanguageEvidence::Unknown,
            &languages(&["en", "pt", "es", "de"]),
        );

        assert_eq!(
            result,
            "so the weather forecast said it would probably rain throughout the whole weekend"
        );
    }

    #[test]
    fn unknown_evidence_with_portuguese_text_preserves_um() {
        let settings = AppSettings {
            selected_language: "auto".to_string(),
            ..Default::default()
        };

        let result = post_process_transcription_text(
            "eu vi um carro na rua ontem de manhã quando fui ao mercado".to_string(),
            &settings,
            false,
            &OutputLanguageEvidence::Unknown,
            &languages(&["en", "pt", "es", "de"]),
        );

        assert_eq!(
            result,
            "eu vi um carro na rua ontem de manhã quando fui ao mercado"
        );
    }

    #[test]
    fn unsupported_explicit_language_uses_model_fallback_as_evidence() {
        let settings = AppSettings {
            selected_language: "pt".to_string(),
            ..Default::default()
        };

        let evidence = resolve_output_language_evidence(&settings, Some("en"));

        assert_eq!(
            evidence,
            OutputLanguageEvidence::ModelConstrained("en".to_string())
        );
    }

    #[test]
    fn language_hint_resolves_auto_to_none() {
        let mut settings = AppSettings {
            selected_language: "auto".to_string(),
            ..Default::default()
        };
        assert_eq!(language_hint(&settings), None);

        settings.selected_language = "de".to_string();
        assert_eq!(language_hint(&settings).as_deref(), Some("de"));

        settings.selected_language = String::new();
        assert_eq!(language_hint(&settings), None);
    }
}
