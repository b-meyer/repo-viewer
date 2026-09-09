//! Typed errors for the engine.
//!
//! Anything a scan can survive is a value rather than an error. A per-repo failure lands on
//! [`crate::model::RepoStatus::error`]; an unreadable root, a permission failure, or a broken
//! `.git` lands on [`crate::model::ScanSummary::errors`]. None of those is a panic and none ends
//! a scan, because one bad path must not cost the user the paths that worked.
//!
//! The variants here are for operations that cannot return a partial result at all: a single
//! named path that turns out not to be a repository, a watcher that cannot be registered, a
//! repository that will not open or whose HEAD is unreadable.
//!
//! That last pair is the line between the two grades of per-repository failure. A repository that
//! cannot be opened or has no readable HEAD produces no row — it stays a
//! [`DiscoveredRepo`](crate::model::DiscoveredRepo) and its failure lands in
//! [`Tier0Summary::errors`](crate::model::Tier0Summary::errors), because every other Tier 0 field
//! is relative to HEAD and a row without one would be invented. A repository that *was* read but
//! whose ahead/behind, stash count, or submodule list failed keeps its row, leaves those fields
//! `None`, and reports the cause on [`RepoStatus::error`](crate::model::RepoStatus::error).

use std::path::PathBuf;

/// Everything that can go wrong inside the engine.
///
/// The `gix`-shaped failures below carry a rendered `String` rather than the originating error.
/// `gix` is pinned pre-1.0 and reshapes its error enums on minor bumps, so wrapping them with
/// `#[from]` would put that churn in this crate's public API — and nothing upstream of here does
/// anything with the cause but display it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A path that should have been a repository was not one.
    #[error("`{0}` is not a git repository")]
    NotARepository(PathBuf),

    /// A repository could not be opened.
    #[error("cannot open `{path}`: {message}")]
    OpenRepo {
        /// The repository's resolved Git directory.
        path: PathBuf,
        /// Rendered cause.
        message: String,
    },

    /// A repository opened, but where its HEAD points could not be determined.
    ///
    /// Total rather than partial: every other Tier 0 field is measured relative to HEAD, so a row
    /// without one would have to invent it.
    #[error("cannot read HEAD of `{path}`: {message}")]
    ReadHead {
        /// The repository's resolved Git directory.
        path: PathBuf,
        /// Rendered cause.
        message: String,
    },

    /// A refs-level read failed on a repository that opened and has a readable HEAD.
    ///
    /// Partial rather than total: the row survives, the affected field stays `None`, and this
    /// message is what reaches [`RepoStatus::error`](crate::model::RepoStatus::error).
    #[error("cannot read {what} of `{path}`: {message}")]
    Refs {
        /// The repository's resolved Git directory.
        path: PathBuf,
        /// Which read failed, for the message — `"ahead/behind"`, `"stash count"`, `"submodules"`.
        what: &'static str,
        /// Rendered cause.
        message: String,
    },

    /// A worktree-level read failed on a repository that opened.
    ///
    /// Partial rather than total: the row keeps everything Tier 0 gave it, the affected Tier 1
    /// field stays unknown, and this message reaches
    /// [`RepoStatus::error`](crate::model::RepoStatus::error). Distinct from [`Error::Refs`]
    /// because it names a read that touched the worktree, which is the expensive kind.
    #[error("cannot read {what} of `{path}`: {message}")]
    Worktree {
        /// The repository's resolved Git directory.
        path: PathBuf,
        /// Which read failed, for the message — `"status"`, `"index"`.
        what: &'static str,
        /// Rendered cause.
        message: String,
    },

    /// A per-repository read panicked and was caught.
    ///
    /// `gix` can panic on a corrupt object or pack. Per-repo work runs under `catch_unwind` so
    /// that becomes this value instead of taking the process down — which is why the release
    /// profile keeps `panic = "unwind"`.
    #[error("panic while reading `{path}`: {message}")]
    Panicked {
        /// The repository being read when the panic happened.
        path: PathBuf,
        /// The panic payload, when it was a string.
        message: String,
    },

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
