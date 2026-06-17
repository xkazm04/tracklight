use thiserror::Error;

#[derive(Debug, Error)]
pub enum BillingError {
    #[error("signature verification failed: {0}")]
    Signature(String),
    #[error("malformed webhook payload: {0}")]
    Parse(String),
}
