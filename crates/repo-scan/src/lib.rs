//! `repo-scan` — Git repository discovery and tiered status reads.
//!
//! This crate is the engine. It has **no Tauri dependency**, which is what makes the boundary in
//! the application structural rather than aspirational: `src-tauri/` is glue (commands, state,
//! channel adaptation), and everything else lives here. If engine code ever needs a Tauri type,
//! the boundary is in the wrong place.
//!
//! It is usable without a GUI — see `examples/scan.rs`, run with
//! `cargo run --release --example scan -- <path>`.
//!
//! # The tiered scan
//!
//! Most of what the dashboard shows costs almost nothing; only file counts are expensive. Three
//! tiers stream independently so a row appears before any worktree is touched:
//!
//! - **Tier 0** — refs only. Branch, ahead/behind, upstream, stash count, state, last commit.
//!   Answers "what have I not pushed?" with zero worktree I/O. This must stay refs-only.
//! - **Tier 1** — the dirty flag, from the status iterator's first item with untracked files
//!   included, plus the conflicted count from index stage entries.
//! - **Tier 2** — full index-to-worktree counts and the submodule list. Lazy: expanded rows and
//!   explicit refresh only, never in the default scan path, and the one tier with no fan-out.

pub mod discover;
pub mod error;
pub mod model;
pub mod status;
pub mod watch;

pub use discover::{canonical, discover_roots, discover_roots_with};
pub use error::{Error, Result};
pub use model::{
    CommitSummary, DEFAULT_PRUNE_NAMES, DiscoveredRepo, FileCounts, Head, RepoEvent, RepoKind,
    RepoState, RepoStatus, ScanError, ScanEvent, ScanId, ScanOpts, ScanSummary, ScanTotals,
    SubmoduleStatus, Tier, Tier0Summary,
};
pub use status::{
    AHEAD_BEHIND_CAP, Tier1, Tier1Summary, Tier2, ahead_behind, read_tier0, read_tier0_all,
    read_tier0_all_with, read_tier1, read_tier1_all_with, read_tier2,
};
pub use watch::{RepoWatcher, WatchEvent, watch_set};
