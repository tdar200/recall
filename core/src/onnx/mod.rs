//! ONNX-backed [`Vad`](crate::vad::Vad) and [`Asr`](crate::asr::Asr) backends:
//! Silero VAD and NVIDIA Parakeet-TDT-0.6B v2, both via the official
//! `sherpa-onnx` crate.
//!
//! Only reachable behind the `onnx` cargo feature. All `sherpa_onnx` types are
//! kept private to this module tree; the public surface is exactly the two
//! constructors plus their trait impls.

mod asr;
mod vad;

pub use asr::ParakeetAsr;
pub use vad::SileroVad;
