//! Typed errors for the engine.
//!
//! Per-repo failures are values on [`crate::model::RepoStatus::error`], never panics and never
//! fatal to a scan. The variants here are for failures that end an operation outright — a root
//! that cannot be walked, a watcher that cannot be registered.

use std::path::PathBuf;

/// Everything that can go wrong inside the engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A configured root could not be read.
    #[error("cannot read root `{path}`: {source}")]
    Root {
        /// The root that failed.
        path: PathBuf,
        /// The underlying I/O failure.
        source: std::io::Error,
    },

    /// A path that should have been a repository was not one.
    #[error("`{0}` is not a git repository")]
    NotARepository(PathBuf),

    /// The filesystem watcher could not be created or could not register a path.
    ///
    /// On Linux this is most often inotify's per-user watch limit, which surfaces as "No space
    /// left on device". Detect that case and print the `sysctl` fix rather than failing opaquely.
    #[error("watch failed: {0}")]
    Watch(#[from] notify::Error),

    /// Any other I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Crate-local result alias. Every public fallible function returns this.
pub type Result<T> = std::result::Result<T, Error>;
