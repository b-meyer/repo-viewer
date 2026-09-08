//! Typed errors for the engine.
//!
//! Anything a scan can survive is a value rather than an error. A per-repo failure lands on
//! [`crate::model::RepoStatus::error`]; an unreadable root, a permission failure, or a broken
//! `.git` lands on [`crate::model::ScanSummary::errors`]. None of those is a panic and none ends
//! a scan, because one bad path must not cost the user the paths that worked.
//!
//! The variants here are for operations that cannot return a partial result at all: a single
//! named path that turns out not to be a repository, a watcher that cannot be registered.

use std::path::PathBuf;

/// Everything that can go wrong inside the engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
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
