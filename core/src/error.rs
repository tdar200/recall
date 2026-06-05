//! Core error type.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Db(String),
    #[error("asr error: {0}")]
    Asr(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("cloud request error: {0}")]
    Cloud(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Db(e.to_string())
    }
}
