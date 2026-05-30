use thiserror::Error;

/// Errors surfaced by core logic. Service crates wrap these in their own error types.
#[derive(Debug, Error)]
pub enum LtError {
    #[error("unknown model: {0}")]
    UnknownModel(String),

    #[error("invalid price book: {0}")]
    InvalidPriceBook(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, LtError>;
