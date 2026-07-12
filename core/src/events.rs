//! Coarse events pushed from the core up to the native shell.
//!
//! This is the upward half of the FFI contract. It will be exported via UniFFI
//! as a foreign trait (task #6) so Kotlin/Swift implement it and the core calls
//! back into it. Only small IDs cross the boundary; the shell then reads full
//! rows from the store. Implementations may be called from any core thread, so
//! they must be thread-safe (`Send + Sync`).

/// Listener implemented by the native shell to receive core events.
///
/// `with_foreign`: UniFFI generates both a Rust-side vtable call path AND the
/// Kotlin/Swift trait implementation glue, so a Kotlin object implementing the
/// generated `RecallListener` interface can be handed back into Rust as an
/// `Arc<dyn RecallListener>` (see `ffi::RecallEngine::new`).
#[uniffi::export(with_foreign)]
pub trait RecallListener: Send + Sync {
    /// A conversation's audio boundaries were finalized and persisted.
    fn on_conversation_finalized(&self, conversation_id: i64);
    /// On-device transcription for a conversation completed.
    fn on_transcript_ready(&self, conversation_id: i64);
    /// A summary (basic extractive, or cloud) for a conversation is ready.
    fn on_summary_ready(&self, conversation_id: i64);
}
