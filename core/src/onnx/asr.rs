//! Parakeet ASR backend (via `sherpa-onnx`) implementing [`crate::asr::Asr`].
//!
//! Runs NVIDIA Parakeet-TDT-0.6B v2 (int8, NeMo transducer export) as an
//! `OfflineRecognizer` in pure batch mode: one call per finalized conversation
//! buffer, never streaming.

use std::path::Path;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

use crate::asr::{Asr, Utterance};
use crate::error::{CoreError, Result};

/// NVIDIA Parakeet-TDT-0.6B v2 batch ASR backend.
///
/// `OfflineRecognizer` is `Send` in the `sherpa-onnx` crate (the underlying C
/// library is thread-safe for single-object usage), so this struct is `Send`
/// automatically, satisfying `Asr: Send`.
pub struct ParakeetAsr {
    recognizer: OfflineRecognizer,
}

impl ParakeetAsr {
    /// Load a Parakeet transducer bundle from `model_dir`, which must contain
    /// `encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx` and
    /// `tokens.txt`.
    pub fn new(model_dir: &Path) -> Result<Self> {
        let encoder = model_dir.join("encoder.int8.onnx");
        let decoder = model_dir.join("decoder.int8.onnx");
        let joiner = model_dir.join("joiner.int8.onnx");
        let tokens = model_dir.join("tokens.txt");

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(encoder.to_string_lossy().into_owned()),
            decoder: Some(decoder.to_string_lossy().into_owned()),
            joiner: Some(joiner.to_string_lossy().into_owned()),
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        config.model_config.model_type = Some("nemo_transducer".to_string());
        config.model_config.provider = Some("cpu".to_string());
        config.model_config.num_threads = 2;

        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
            CoreError::Asr(format!(
                "failed to create sherpa-onnx OfflineRecognizer from {}",
                model_dir.display()
            ))
        })?;

        Ok(Self { recognizer })
    }
}

impl Asr for ParakeetAsr {
    fn transcribe(&mut self, pcm: &[i16]) -> Result<Vec<Utterance>> {
        if pcm.is_empty() {
            return Ok(vec![]);
        }

        let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();

        let stream = self.recognizer.create_stream();
        stream.accept_waveform(16_000, &samples);
        self.recognizer.decode(&stream);

        let result = stream
            .get_result()
            .ok_or_else(|| CoreError::Asr("sherpa-onnx returned no recognition result".into()))?;

        let text = result.text.trim().to_string();
        if text.is_empty() {
            return Ok(vec![]);
        }

        // sherpa's token timestamps are token-level, not word-level; a single
        // utterance spanning the whole buffer is acceptable for v1 (per task brief).
        Ok(vec![Utterance {
            start_ms: 0,
            end_ms: (pcm.len() as u64) / 16,
            text,
        }])
    }
}
