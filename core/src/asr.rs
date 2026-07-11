//! Speech-to-text abstraction.
//!
//! The production backend is NVIDIA Parakeet-TDT-0.6B v2 via the official
//! `sherpa-onnx` crate ([`crate::onnx::ParakeetAsr`], behind the `onnx` cargo
//! feature), run in BATCH on each finalized conversation — never streaming.
//! The core depends only on the [`Asr`] trait so host tests can use a mock.

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

/// An [`Asr`] that never transcribes anything.
///
/// Exists for two device-side cases: M0 soak mode (where `EngineCore` already
/// skips ASR entirely, but the shell still needs a concrete `Box<dyn Asr>` to
/// construct the engine) and the no-onnx fallback build, where the real
/// Parakeet backend isn't linked in.
pub struct NoopAsr;

impl Asr for NoopAsr {
    fn transcribe(&mut self, _pcm: &[i16]) -> Result<Vec<Utterance>> {
        Ok(vec![])
    }
}
