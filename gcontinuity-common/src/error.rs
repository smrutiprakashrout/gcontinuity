use thiserror::Error;

#[derive(Error, Debug)]
pub enum GContinuityError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Unknown error: {0}")]
    Unknown(String),
}
