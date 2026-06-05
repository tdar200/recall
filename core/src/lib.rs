//! Recall shared core.
//!
//! Pure, device-independent engine for the Recall app: it receives PCM audio
//! (pushed down in large chunks from the native shell), gates it with on-device
//! VAD, splits it into conversations, transcribes them on-device, stores them,
//! and produces summaries. The native shells (Kotlin now, Swift later) own only
//! the microphone foreground service, OS lifecycle, and UI.
//!
//! Architecture rule: nothing high-frequency crosses the FFI. The shell hands
//! down large PCM chunks; the core emits only coarse events upward.
//!
//! The ONNX-backed VAD (Silero) and ASR (Parakeet via sherpa-onnx) live behind
//! the [`vad::Vad`] and [`asr::Asr`] traits so this crate builds and is fully
//! unit-testable on the host without the Android NDK / native ONNX libraries.

pub mod asr;
pub mod audio;
pub mod cloud;
pub mod engine;
pub mod error;
pub mod events;
pub mod model;
pub mod segment;
pub mod store;
pub mod summary;
pub mod vad;

pub use error::{CoreError, Result};
pub use model::{ConversationSpan, Millis};
