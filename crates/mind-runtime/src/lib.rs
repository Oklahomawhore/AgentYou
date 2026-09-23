//! Phase A: durable, single-writer, shadow-only cognition runtime.
pub mod model;
pub mod policy;
pub mod runtime;
pub mod store;
pub use model::*;
pub use runtime::Runtime;
pub use store::Store;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("serialization: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("runtime stopped")]
    Stopped,
    #[error("storage I/O: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
