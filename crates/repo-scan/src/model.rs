//! The wire model.
//!
//! Three rules hold every type in this file, and breaking any of them breaks the frontend:
//!
//! 1. **No `gix` types.** Object ids are hex `String`, not `gix::ObjectId`. The engine's Git
//!    library must not leak across the IPC boundary.
//! 2. **`ts-rs`-expressible.** Times are `u64` epoch milliseconds, not `SystemTime` — `ts-rs` has
//!    no impl for it and serde would emit a `secs`/`nanos` struct. `TS_RS_LARGE_INT=number` in
//!    `.cargo/config.toml` keeps those `u64`s from generating as `bigint`.
//! 3. **Every tiered field is `Option`.** `None` means "not computed yet", and the UI must render
//!    that as unknown rather than as `0`. Showing `0` for an uncomputed count is the most common
//!    bug in this class of app, and the `Option` is what makes it impossible to write by accident.
//!
//! `ts-rs` mirrors serde attributes via its default `serde-compat` feature, so `rename_all` and
//! `tag` are honoured without duplicating them as `#[ts(...)]`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What kind of repository a row describes.
///
/// Bare repos have no worktree, so their tiered fields stay `None` forever. Linked worktrees each
/// get their own row with their own HEAD and index, but share one object store — they must not be
/// counted as separate repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub enum RepoKind {
    /// An ordinary repository with a worktree and a `.git` directory.
    Normal,
    /// No worktree; detected via `HEAD` + `objects/` + `refs/` at the root.
    Bare,
    /// A linked worktree, whose `.git` is a file containing `gitdir: <path>`.
    LinkedWorktree,
    /// A submodule, whose `.git` is likewise a file.
    Submodule,
}

/// An in-progress operation that changes what actions make sense on a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub enum RepoState {
    /// No operation in progress.
    Clean,
    /// A merge is in progress.
    Merging,
    /// A rebase is in progress.
    Rebasing,
    /// A bisect is in progress.
    Bisecting,
    /// A cherry-pick is in progress.
    CherryPicking,
}

/// Where HEAD points.
///
/// Internally tagged so the generated TypeScript is a discriminated union the frontend can narrow
/// on, rather than a struct with three optional halves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub enum Head {
    /// HEAD is on a branch.
    #[serde(rename_all = "camelCase")]
    Branch {
        /// Short branch name, e.g. `main`.
        name: String,
    },
    /// HEAD points directly at a commit.
    #[serde(rename_all = "camelCase")]
    Detached {
        /// Hex commit id.
        id: String,
    },
    /// A repository with no commits yet — HEAD names a branch that does not exist.
    Unborn,
}

/// Just enough of the tip commit to render a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct CommitSummary {
    /// Hex commit id.
    pub id: String,
    /// First line of the commit message.
    pub summary: String,
    /// Author name as recorded in the commit.
    pub author: String,
    /// Author time, epoch milliseconds.
    pub time_ms: u64,
}

/// Tier 2 file counts. Only ever populated on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct FileCounts {
    /// Paths staged for commit.
    pub staged: u32,
    /// Tracked paths modified in the worktree.
    pub unstaged: u32,
    /// Paths not tracked and not ignored.
    pub untracked: u32,
    /// Paths with conflict markers in the index.
    pub conflicted: u32,
}

/// A submodule as recorded by its parent.
///
/// Enumerated from the parent's config rather than by walking, so a submodule is never discovered
/// twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct SubmoduleStatus {
    /// Submodule name from `.gitmodules`.
    pub name: String,
    /// Path relative to the parent worktree.
    pub path: PathBuf,
    /// Commit the parent records for it, hex.
    pub recorded_id: Option<String>,
    /// Commit the submodule's own HEAD is at, hex. `None` when it is not checked out.
    pub head_id: Option<String>,
}

/// One row of the dashboard: everything known about a repository, merged across tiers.
///
/// Rust owns the canonical copy of this (`src-tauri/src/state.rs` holds the one
/// `HashMap<PathBuf, RepoStatus>`), merges each tier into it field-wise, and sends the **full
/// merged row** over the channel. The Pinia store is a mirror keyed by path — it never merges and
/// never holds a value Rust does not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct RepoStatus {
    /// Absolute path to the worktree (or to the repository itself, when bare). The map key.
    pub path: PathBuf,
    /// Directory name, for display.
    pub name: String,
    /// Parent directory, so the frontend can group by folder without doing path manipulation.
    pub parent: PathBuf,
    /// Which flavour of repository this is.
    pub kind: RepoKind,

    // ---- Tier 0: refs only. Sub-millisecond per repo; no worktree I/O. ----
    /// Where HEAD points.
    pub head: Head,
    /// Upstream tracking ref, e.g. `origin/main`. `None` when none is configured.
    pub upstream: Option<String>,
    /// Commits ahead of upstream. `None` when there is no upstream. Capped; see the walk cap.
    pub ahead: Option<u32>,
    /// Commits behind upstream. `None` when there is no upstream.
    pub behind: Option<u32>,
    /// The tip commit.
    pub last_commit: Option<CommitSummary>,
    /// Number of stash entries.
    pub stash_count: u32,
    /// In-progress operation, if any.
    pub state: RepoState,
    /// Modification time of `FETCH_HEAD`, epoch milliseconds. `None` when never fetched.
    ///
    /// Ahead/behind is measured against `refs/remotes/origin/*`, which is only as fresh as this.
    /// Never present ahead/behind without it.
    pub last_fetched_ms: Option<u64>,

    // ---- Tier 1: dirty flag. Early-exit on the first status item. ----
    /// Whether the worktree differs from HEAD, **including untracked files**. `None` = not yet
    /// computed.
    pub dirty: Option<bool>,
    /// Index entries with stage > 0. `None` = not yet computed.
    pub conflicted: Option<u32>,

    // ---- Tier 2: full counts. Lazy — expanded rows and explicit refresh only. ----
    /// Full index-to-worktree counts. `None` = not yet computed.
    pub counts: Option<FileCounts>,
    /// Submodules recorded by this repository.
    pub submodules: Vec<SubmoduleStatus>,

    /// When this row was last read, epoch milliseconds. Rendered as an age until refreshed.
    pub scanned_at_ms: u64,
    /// A per-repo failure. Never fatal to a scan, and never a panic.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------------------------

/// A repository the walk found, before any Git read has happened.
///
/// Deliberately **not** a partial [`RepoStatus`]: that type's Tier 0 fields (`head`, `state`,
/// `stash_count`) are not `Option`, because a row that has been read always has them. Discovery
/// has read nothing, so it cannot honestly produce one. This is what the `RepoFound` event
/// carries; Tier 0 turns it into a `RepoStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct DiscoveredRepo {
    /// Absolute path to the worktree, or to the repository itself when bare.
    ///
    /// Canonicalised through `dunce`, so it carries the on-disk casing and no verbatim prefix. This
    /// is the deduplication key during a scan and the `HashMap` key afterwards.
    pub path: PathBuf,
    /// Directory name, for display.
    pub name: String,
    /// Parent directory, so the frontend can group by folder without doing path manipulation.
    pub parent: PathBuf,
    /// Which flavour of repository this is.
    pub kind: RepoKind,
    /// The resolved Git directory: `<path>/.git` for a normal repo, `<path>` itself when bare,
    /// and the private directory the `.git` _file_ points at for a worktree or submodule.
    ///
    /// Resolved here so later phases never re-resolve it — Tier 0 opens it, and the watcher
    /// (§7.2) registers its watch set against it.
    pub git_dir: PathBuf,
}

/// Directory names pruned by default: only names that are near-certainly generated.
///
/// `bin`, `obj`, `build`, `dist` and `vendor` are **deliberately absent**. A Go `vendor/` tree or
/// a `Source/build/` folder legitimately holds repositories, and a name-pruned repository
/// vanishes with no way for the user to notice. [`ScanSummary::dirs_pruned`] exists so the
/// omissions this list *does* cause stay visible.
pub const DEFAULT_PRUNE_NAMES: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".gradle",
    ".terraform",
    "Pods",
    ".next",
    ".nuxt",
];

/// How to walk. Every field has a default, and `serde(default)` lets the frontend send a subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct ScanOpts {
    /// How deep to descend below each root. `None` is unlimited.
    ///
    /// `u32` rather than `usize` so the generated TypeScript is a plain `number`; a depth beyond
    /// `u32::MAX` is not a real configuration.
    pub max_depth: Option<u32>,
    /// Do not cross filesystem boundaries. On by default: descending into a network share or a
    /// mounted volume is a common cause of multi-minute scans.
    pub same_file_system: bool,
    /// Follow symlinks. Off by default — symlink loops are real, and Windows junctions and
    /// reparse points are already reported as symlinks by `std`, so this covers them too.
    pub follow_links: bool,
    /// Walker thread count. `None` derives it; see `discover` for what it derives.
    pub threads: Option<u32>,
    /// Directory names to prune. Defaults to [`DEFAULT_PRUNE_NAMES`].
    pub prune_names: Vec<String>,
    /// Keep descending after finding a repository, so genuinely independent nested checkouts are
    /// found too.
    ///
    /// Off by default: the walk stops at the first `.git`. Submodules are unaffected either way —
    /// they are enumerated from the parent's config rather than found by walking.
    pub descend_into_repos: bool,
}

impl Default for ScanOpts {
    fn default() -> Self {
        Self {
            max_depth: Some(8),
            same_file_system: true,
            follow_links: false,
            threads: None,
            prune_names: DEFAULT_PRUNE_NAMES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            descend_into_repos: false,
        }
    }
}

/// A path the scan could not read, kept as a value rather than raised.
///
/// Permission errors are the common case and are never fatal to a scan (§5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct ScanError {
    /// The path that failed.
    pub path: PathBuf,
    /// Rendered cause. A string because this crosses IPC.
    pub message: String,
}

/// What a completed walk did, as opposed to what it found.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct ScanSummary {
    /// Repositories recorded, after deduplication.
    pub repos_found: u32,
    /// Directories visited. The denominator for the walk's cost.
    pub dirs_visited: u32,
    /// Directories skipped by the prune predicate.
    ///
    /// Surfaced in the UI so a name-based omission is visible rather than silent.
    pub dirs_pruned: u32,
    /// Paths that could not be read. Never fatal.
    pub errors: Vec<ScanError>,
    /// Wall-clock duration of the walk, milliseconds.
    pub elapsed_ms: u64,
}
