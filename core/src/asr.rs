//! Speech-to-text abstraction.
//!
//! The production backend is NVIDIA Parakeet-TDT-0.6B v2 via sherpa-onnx
//! (the `sherpa-rs` crate), run in BATCH on each finalized conversation —
//! never streaming. It is added on the Android NDK build; the core depends only
//! on the [`Asr`] trait so host tests can use a mock.

use crate::error::Result;

/// One transcribed utterance within a conversation, with timestamps relative to
/// the start of the conversation's PCM buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct Utterance {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// A batch speech-to-text backend.
pub trait Asr: Send {
    /// Transcribe a finalized PCM buffer (i16 mono) in a single batch call.
    fn transcribe(&mut self, pcm: &[i16]) -> Result<Vec<Utterance>>;
}
