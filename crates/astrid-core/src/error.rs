//! The single error type for the whole workspace.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("provider: {0}")]
    Provider(String),

    #[error("tool refused ({tool}): {reason}")]
    ToolRefused { tool: String, reason: String },

    #[error("memory: {0}")]
    Memory(String),

    #[error("confinement: {0}")]
    Confinement(String),

    #[error("hook blocked: {0}")]
    HookBlocked(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
