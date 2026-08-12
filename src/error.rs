//! Error types shared across the crate.
//!
//! One enum, no external error crates. Every I/O error carries the path and
//! the operation that failed so diagnostics always say *where*.

use std::fmt;
use std::io;
use std::path::Path;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, GitError>;

/// The single error type for all git-rs failures.
#[derive(Debug)]
pub enum GitError {
    /// An object, ref, or path that was expected to exist does not.
    NotFound(String),
    /// A format violation or integrity failure discovered while reading.
    Corrupt(String),
    /// Invalid user input: bad arguments, bad ref names, bad config values.
    Invalid(String),
    /// A user-input or command failure real git reports as a fatal error
    /// (exit 128), e.g. ref update failures.
    Fatal(String),
    /// An I/O failure with the path and operation that failed.
    Io {
        /// The path that failed to read or write.
        path: String,
        /// What we were doing with it.
        op: String,
        /// The underlying I/O error.
        source: io::Error,
    },
}

impl GitError {
    /// Wrap an I/O error with the path and operation that failed.
    pub fn io(path: impl Into<String>, op: impl Into<String>, source: io::Error) -> Self {
        GitError::Io {
            path: path.into(),
            op: op.into(),
            source,
        }
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::NotFound(msg) => write!(f, "{msg}"),
            GitError::Corrupt(msg) => write!(f, "{msg}"),
            GitError::Invalid(msg) => write!(f, "{msg}"),
            GitError::Fatal(msg) => write!(f, "{msg}"),
            GitError::Io { path, op, source } => write!(f, "{op} failed for '{path}': {source}"),
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for GitError {
    /// Fallback for `?` on raw I/O results; prefer `.context(path, op)` so
    /// every error names the path it came from.
    fn from(source: io::Error) -> Self {
        GitError::Io {
            path: "<unknown>".into(),
            op: "<unknown>".into(),
            source,
        }
    }
}

/// Extension trait adding `.context(path, op)` to `io::Result`.
pub trait IoContext<T> {
    /// Attach the path and operation to an I/O error.
    fn context<P: AsRef<Path>, O: AsRef<str>>(self, path: P, op: O) -> Result<T>;
}

impl<T> IoContext<T> for std::result::Result<T, io::Error> {
    fn context<P: AsRef<Path>, O: AsRef<str>>(self, path: P, op: O) -> Result<T> {
        self.map_err(|source| {
            GitError::io(
                path.as_ref().display().to_string(),
                op.as_ref().to_string(),
                source,
            )
        })
    }
}
