//! Core error type.

use thiserror::Error;

/// `flat_error`: UniFFI exposes each variant to Kotlin/Swift carrying only its
/// `Display` message (via `thiserror`'s `#[error(...)]`), not its fields —
/// simpler foreign-side error handling than mirroring every field.
#[derive(Debug, Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Db(String),
    #[error("asr error: {0}")]
    Asr(String),
    #[error("vad error: {0}")]
    Vad(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("cloud request error: {0}")]
    Cloud(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Db(e.to_string())
    }
}
