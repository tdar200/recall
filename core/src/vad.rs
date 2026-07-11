//! Voice-activity-detection abstraction.
//!
//! The production backend is Silero VAD v5 via the official `sherpa-onnx`
//! crate ([`crate::onnx::SileroVad`], behind the `onnx` cargo feature). The
//! core and its tests depend only on the [`Vad`] trait plus the per-frame
//! [`VadFrame`] values that drive the segmentation FSM.

/// The result of analysing one fixed-size audio frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadFrame {
    /// Probability in `[0, 1]` that the frame contains speech.
    pub speech_prob: f32,
    /// A normalized energy/RMS proxy for the frame, used by the segmenter's
    /// secondary gate to reject loud-but-non-speech triggers.
    pub energy: f32,
}

/// A voice-activity-detection backend over fixed-size PCM chunks.
///
/// Implementations are stateful in streaming mode (Silero carries recurrent
/// state across chunks) and must be [`Vad::reset`] at conversation boundaries.
pub trait Vad: Send {
    /// Process one fixed-size PCM chunk (i16 mono at the configured sample
    /// rate; Silero requires 512 samples @ 16 kHz) and return its speech
    /// probability in `[0, 1]`.
    fn process_chunk(&mut self, pcm: &[i16]) -> f32;

    /// Reset recurrent state at a stream/conversation boundary.
    fn reset(&mut self);
}
