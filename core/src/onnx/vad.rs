//! Silero VAD backend (via `sherpa-onnx`) implementing [`crate::vad::Vad`].
//!
//! sherpa-onnx exposes no raw per-frame speech probability — only the
//! post-hysteresis `VoiceActivityDetector` state machine (`accept_waveform`
//! 512-sample frames, poll `detected()`). We adapt that boolean into the
//! `[0, 1]` probability our trait expects (1.0 / 0.0); the engine's own
//! segmenter applies its own enter/exit hysteresis on top, so sherpa's
//! internal smoothing is kept minimal (short min-speech/min-silence windows).

use std::path::Path;

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use crate::error::{CoreError, Result};
use crate::vad::Vad;

/// Silero VAD v5 backend. Stateful across `process_chunk` calls; call
/// [`Vad::reset`] at conversation/stream boundaries.
///
/// `VoiceActivityDetector` is `Send` in the `sherpa-onnx` crate (the
/// underlying C library is thread-safe for single-object usage), so this
/// struct is `Send` automatically, satisfying `Vad: Send`.
pub struct SileroVad {
    inner: VoiceActivityDetector,
}

impl SileroVad {
    /// Load a Silero VAD ONNX model (e.g. `silero_vad.onnx`) from `model_path`.
    pub fn new(model_path: &Path) -> Result<Self> {
        let silero_vad = SileroVadModelConfig {
            model: Some(model_path.to_string_lossy().into_owned()),
            threshold: 0.5,
            min_silence_duration: 0.1,
            min_speech_duration: 0.05,
            window_size: 512,
            max_speech_duration: 20.0,
        };
        let config = VadModelConfig {
            silero_vad,
            ten_vad: Default::default(),
            sample_rate: 16_000,
            num_threads: 1,
            provider: Some("cpu".to_string()),
            debug: false,
        };

        let inner = VoiceActivityDetector::create(&config, 30.0).ok_or_else(|| {
            CoreError::Vad(format!(
                "failed to create sherpa-onnx VoiceActivityDetector from {}",
                model_path.display()
            ))
        })?;

        Ok(Self { inner })
    }
}

impl Vad for SileroVad {
    fn process_chunk(&mut self, pcm: &[i16]) -> f32 {
        let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
        self.inner.accept_waveform(&samples);
        if self.inner.detected() {
            1.0
        } else {
            0.0
        }
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}
