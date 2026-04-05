use std::io;

use thiserror::Error;

pub type TTResult<T> = Result<T, TTError>;

#[derive(Debug, Error)]
pub enum TTError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("store error: {0}")]
    Store(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("serialization error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    #[error("toml decode error: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("toml encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),
}
