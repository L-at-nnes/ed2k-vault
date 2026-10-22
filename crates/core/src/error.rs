//! Error type shared by the core modules.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {cause}", .path.display())]
    Io {
        path: PathBuf,
        cause: std::io::Error,
    },
    #[error("{}: file changed while it was being read", .0.display())]
    Modified(PathBuf),
    #[error("operation cancelled")]
    Cancelled,
    #[error("cache error: {0}")]
    Cache(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: impl Into<PathBuf>, cause: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            cause,
        }
    }
}
