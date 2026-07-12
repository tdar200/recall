//! Host smoke test for the UniFFI bridge (`core/src/ffi.rs`), run WITHOUT the
//! `onnx` feature (the default `cargo test`). It only exercises the one path
//! that's meaningful without real ONNX models: `RecallEngine::new` must fail
//! with a clear, explanatory error when no VAD backend is linked in — see the
//! `#[cfg(not(feature = "onnx"))]` arm of `ffi::build_vad_asr`.
//!
//! Byte↔i16 PCM conversion is unit-tested in `ffi.rs` itself (it's `pub(crate)`,
//! not reachable from an external integration test).

#![cfg(not(feature = "onnx"))]

use std::sync::Arc;

use recall_core::events::RecallListener;
use recall_core::ffi::RecallEngine;

struct NoopListener;
impl RecallListener for NoopListener {
    fn on_conversation_finalized(&self, _conversation_id: i64) {}
    fn on_transcript_ready(&self, _conversation_id: i64) {}
    fn on_summary_ready(&self, _conversation_id: i64) {}
}

#[test]
fn constructing_without_onnx_feature_returns_an_explanatory_error() {
    let dir = std::env::temp_dir().join(format!("recall-ffi-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let result = RecallEngine::new(
        dir.join("recall.db").to_string_lossy().into_owned(),
        dir.join("audio").to_string_lossy().into_owned(),
        dir.join("models").to_string_lossy().into_owned(),
        1_000_000,
        false,
        false,
        Arc::new(NoopListener),
    );
    let err = match result {
        Ok(_) => panic!("RecallEngine::new must fail without the onnx feature"),
        Err(e) => e,
    };

    let msg = err.to_string();
    assert!(
        msg.contains("onnx") || msg.contains("VAD"),
        "expected an explanatory onnx/VAD error, got: {msg}"
    );
}
