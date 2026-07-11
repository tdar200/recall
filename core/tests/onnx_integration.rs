//! Host integration tests for the ONNX-backed `SileroVad` + `ParakeetAsr`
//! backends, run against the real models fetched by `scripts/fetch-models.sh`
//! into `models/`.
//!
//! Requires `--features onnx` and the models to be present; run:
//!   cargo test --features onnx --test onnx_integration

#![cfg(feature = "onnx")]

use std::path::{Path, PathBuf};

use recall_core::asr::Asr;
use recall_core::onnx::{ParakeetAsr, SileroVad};
use recall_core::vad::Vad;

const FRAME_SAMPLES: usize = 512;

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("models")
}

fn silero_model_path() -> PathBuf {
    models_dir().join("silero_vad.onnx")
}

fn parakeet_model_dir() -> PathBuf {
    models_dir().join("parakeet")
}

fn test_wav_path() -> PathBuf {
    parakeet_model_dir().join("test_wavs").join("0.wav")
}

/// Read a real speech wav bundled with the Parakeet model via sherpa-onnx's
/// own `Wave` reader (the crate is already a test-only dependency behind the
/// `onnx` feature, so this doubles as our "existing wav-reading helper"),
/// converting its normalized f32 PCM back to i16 to match our `Vad`/`Asr`
/// trait surface (which the engine always feeds i16).
fn read_test_wav_as_i16() -> Vec<i16> {
    let path = test_wav_path();
    let wave = sherpa_onnx::Wave::read(path.to_str().expect("utf8 path"))
        .unwrap_or_else(|| panic!("failed to read test wav at {}", path.display()));
    assert_eq!(wave.sample_rate(), 16_000, "test wav must be 16 kHz mono");
    wave.samples()
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect()
}

#[test]
fn silero_detects_speech_vs_silence() {
    let pcm = read_test_wav_as_i16();
    assert!(pcm.len() >= FRAME_SAMPLES, "test wav too short");

    let mut vad = SileroVad::new(&silero_model_path()).expect("load SileroVad");

    // Mean speech probability over the real (voiced) wav, fed in Silero's
    // fixed 512-sample frames like the engine does.
    let mut probs = Vec::new();
    for chunk in pcm.chunks(FRAME_SAMPLES) {
        if chunk.len() < FRAME_SAMPLES {
            break; // partial trailing frame; engine never feeds these
        }
        probs.push(vad.process_chunk(chunk));
    }
    assert!(!probs.is_empty(), "expected at least one full frame");
    let mean_speech: f32 = probs.iter().sum::<f32>() / probs.len() as f32;
    assert!(
        mean_speech > 0.5,
        "expected mean speech prob > 0.5 over voiced wav, got {mean_speech}"
    );

    // Mean probability over pure silence (zero) frames should be low.
    vad.reset();
    let silent_frame = [0i16; FRAME_SAMPLES];
    let mut silent_probs = Vec::new();
    for _ in 0..20 {
        silent_probs.push(vad.process_chunk(&silent_frame));
    }
    let mean_silence: f32 = silent_probs.iter().sum::<f32>() / silent_probs.len() as f32;
    assert!(
        mean_silence < 0.1,
        "expected mean speech prob < 0.1 over silence, got {mean_silence}"
    );
}

#[test]
fn parakeet_transcribes_test_wav() {
    let pcm = read_test_wav_as_i16();

    let mut asr = ParakeetAsr::new(&parakeet_model_dir()).expect("load ParakeetAsr");
    let utterances = asr.transcribe(&pcm).expect("transcribe");

    assert!(!utterances.is_empty(), "expected at least one utterance");
    let u = &utterances[0];
    assert!(u.end_ms > u.start_ms, "utterance must have positive duration");
    assert!(!u.text.trim().is_empty(), "utterance text must be non-empty");

    // Pinned by one manual reference run of ParakeetAsr against
    // models/parakeet/test_wavs/0.wav (2026-07-12): the recognizer transcribes
    // it as "well, i don't wish to see it any more, observed phebe, turning
    // away her eyes. it is certainly very like the old portrait." — "portrait"
    // is a distinctive, unambiguous word from that transcript.
    let lower = u.text.to_lowercase();
    assert!(
        lower.contains("portrait"),
        "expected transcript to contain the pinned reference word 'portrait', got: {lower:?}"
    );
}
