pub mod audio;
pub mod chunking;
pub mod constants;
pub(crate) mod cpu_engine;
pub mod lang_id;
pub mod text;
pub mod utils;
pub mod vad;

// sttts: fork keeps its cpal recorder stack (device/recorder exports); the
// sink/capture stack stays deleted. decode_audio_file feeds the upstream
// file-transcription feature (history → remote transcription).
pub use audio::{
    decode_audio_file, is_microphone_access_denied, is_no_input_device_error, list_input_devices,
    list_output_devices, read_wav_samples, save_wav_file, verify_wav_file, AudioRecorder,
    CpalDeviceInfo, VadPolicy,
};
pub use lang_id::detect_output_language;
pub use text::{
    apply_custom_words, normalize_transcription_output, remove_filler_words, OutputLanguageEvidence,
};
pub use utils::get_cpal_host;
pub use vad::{EarshotVad, SileroVad, SpeechScorer, VoiceActivityDetector};
