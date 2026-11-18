//! sttts: type stub of the upstream language_detection module.
//!
//! The fork transcribes remotely, so there is no local detector here. What
//! remains are the types History persists per entry (how a transcription's
//! language was chosen) — kept byte-compatible with upstream's storage
//! format so a future local-detector adoption is a pure extension.

use serde::{Deserialize, Serialize};
use specta::Type;

/// How a transcription's language was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LanguageSource {
    /// The person picked it, or it is the only language of theirs the model
    /// supports.
    Selected,
    /// The model decides: its own detection, or its only language.
    Model,
    /// Handy's detector picked it.
    Detected,
    /// Handy had to pick without detecting (too little speech, detection
    /// failed or unavailable, or Auto can't work for these languages).
    Fallback,
}

impl LanguageSource {
    /// How it is stored in history.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::Model => "model",
            Self::Detected => "detected",
            Self::Fallback => "fallback",
        }
    }

    /// Reads [`as_str`](Self::as_str); `None` for anything else (a newer
    /// Handy's value).
    pub fn parse(value: &str) -> Option<Self> {
        [Self::Selected, Self::Model, Self::Detected, Self::Fallback]
            .into_iter()
            .find(|source| source.as_str() == value)
    }
}

/// The language one transcription runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageDecision {
    /// The model's code, or `None` for the model's own detection.
    pub language: Option<String>,
    pub source: LanguageSource,
}

impl LanguageDecision {
    /// As a language intent: the code, or `"auto"`.
    pub fn intent(&self) -> &str {
        self.language.as_deref().unwrap_or("auto")
    }
}
