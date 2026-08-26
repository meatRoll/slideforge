//! Crate-wide error type.

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by SlideForge.
#[derive(Debug, Error)]
pub enum Error {
    /// A filesystem operation failed while accessing `path`.
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    /// The YAML in `path` could not be deserialized into PPTD types.
    #[error("failed to parse {path} as PPTD: {source}")]
    Yaml {
        path: PathBuf,
        source: serde_yaml::Error,
    },

    /// The document is structurally invalid (wrong version, bad fields, ...).
    #[error("invalid PPTD: {0}")]
    Invalid(String),

    /// The document parsed but failed semantic validation.
    #[error("validation failed: {0}")]
    Validation(String),

    /// A parsed construct the writer does not support yet.
    #[error("not supported yet: {0}")]
    Unsupported(String),

    /// Packaging (ZIP) or writing the output failed.
    #[error("failed to write PPTX: {0}")]
    Zip(String),
}

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;
