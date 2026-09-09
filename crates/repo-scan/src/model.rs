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
///
/// `gix` distinguishes ten operations; these six collapse the ones a dashboard treats alike. A
/// mailbox application and an interactive rebase are both "rebasing" to a reader deciding whether
/// a repository is safe to touch, and the sequence variants differ from their single-commit form
/// only in how many commits remain. Nothing is folded into `Clean`: an operation in progress must
/// never render as no operation, which is why mapping `gix`'s enum has no wildcard arm — a new
/// variant upstream becomes a compile error rather than a silent "nothing going on".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub enum RepoState {
    /// No operation in progress.
    Clean,
    /// A merge is in progress.
    Merging,
    /// A rebase is in progress, including a mailbox application and an interactive rebase.
    Rebasing,
    /// A bisect is in progress.
    Bisecting,
    /// A cherry-pick is in progress, whether one commit or a sequence.
    CherryPicking,
    /// A revert is in progress, whether one commit or a sequence.
    Reverting,
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
///
/// Wholly a Tier 2 value, because none of it can be had from refs. The name and path come from
/// `.gitmodules`, which is a worktree file and falls back to a full index parse when missing;
/// `recorded_id` is an index entry; and `head_id` means opening the submodule's own repository.
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
    ///
    /// `Some` here with `ahead`/`behind` both `None` is a real and distinct state: the branch has
    /// an upstream configured, but no local `refs/remotes/*` ref to count against — never fetched,
    /// or the remote branch was deleted. The name is worth showing; the counts would be invented.
    pub upstream: Option<String>,
    /// Commits ahead of upstream. `None` when there is no upstream configured.
    ///
    /// Counted against `refs/remotes/*`, so it is only as fresh as `last_fetched_ms` — never
    /// present one without the other. A value equal to
    /// [`AHEAD_BEHIND_CAP`](crate::status::AHEAD_BEHIND_CAP) means "at least that many" and
    /// renders as `1000+`: the walk stops there because disjoint histories can otherwise traverse
    /// every commit in the repository.
    pub ahead: Option<u32>,
    /// Commits behind upstream. `None` when there is no upstream configured. Capped like `ahead`.
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

    /// Submodules recorded by this repository, enumerated from its config rather than by walking.
    ///
    /// `None` = not yet computed; `Some(vec![])` = read, and there are none. An empty `Vec` alone
    /// could not tell those apart, which is the same mistake as rendering an uncomputed count
    /// as `0`.
    ///
    /// Tier 2, in full. Reading it is not a refs operation at any granularity: `.gitmodules` is a
    /// worktree file, and when it is absent the lookup falls back to parsing the whole index.
    pub submodules: Option<Vec<SubmoduleStatus>>,

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
/// has read nothing, so it cannot honestly produce one. This is what the [`ScanEvent::ReposFound`]
/// event carries; Tier 0 turns it into a `RepoStatus`.
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

/// What a completed Tier 0 pass did, as opposed to what it read.
///
/// Deliberately shaped like [`ScanSummary`]: a count, the failures as values, and a duration. The
/// errors here are repositories that could not be read *at all* — one that was read but whose
/// ahead/behind or stash count failed carries its own message on [`RepoStatus::error`] instead and
/// is counted in `repos_read`.
///
/// **Not `ts(export)`ed, unlike its siblings**, because it does not cross IPC: one Tier 0 pass is a
/// per-batch detail of the pipeline, and the scan reports [`ScanTotals`] instead. Exporting it
/// anyway would put a file in `src/scripts/generated/` that nothing imports, in a directory whose
/// whole claim is that it mirrors the wire.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tier0Summary {
    /// Repositories that produced a row.
    pub repos_read: u32,
    /// Repositories that could not be opened or whose HEAD could not be read. Never fatal.
    pub errors: Vec<ScanError>,
    /// Wall-clock duration of the pass, milliseconds.
    pub elapsed_ms: u64,
}

/// How much of a row to read.
///
/// Cumulative: a tier names itself **and every cheaper tier below it**, because the tiers are not
/// independent. Tier 1 without Tier 0 would produce a `dirty` flag for a row with no `head` to
/// attach it to, and Tier 2 alone could not tell a repository that moved from one that did not.
/// So `Two` means "read all three", which is what a refresh of a single row wants.
///
/// Variants are named for the numbers rather than as `Tier0`/`Tier1`/`Tier2`: repeating the type's
/// own name in every variant trips `clippy::enum_variant_names`, which `-D warnings` makes fatal.
/// `Ord` so the gate reads `tier >= Tier::One` rather than as a match with three arms.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub enum Tier {
    /// Refs only: branch, upstream, ahead/behind, stash, state, tip commit, last-fetched age.
    #[default]
    Zero,
    /// Tier 0, plus the dirty flag and the conflicted count.
    One,
    /// Tier 0 and 1, plus the full file counts and the submodule list.
    Two,
}

/// What a whole scan did, tier by tier.
///
/// [`ScanEvent::Finished`]'s payload, and deliberately **not** [`Tier0Summary`]: reusing that type
/// meant one `elapsed_ms` field standing for the whole pipeline, so anything rendering it could
/// only honestly say "scan" — which blamed the cheap tier for the expensive one's cost, given
/// Tier 1 runs about 24x Tier 0 cold.
///
/// `elapsed_ms` is wall clock across the walk and every tier. The three per-stage fields are sums
/// of the per-batch passes, which is a different measurement and the right one for attribution:
/// summing them would *understate* a total, because it omits the time spent waiting for the walk
/// to produce the next batch, and that wait is most of what a user experiences. The batches run
/// sequentially, so the spans do not overlap and the sums do not double-count.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct ScanTotals {
    /// Repositories that produced a row.
    pub repos_read: u32,
    /// Repositories that could not be opened or whose HEAD could not be read. Never fatal.
    ///
    /// Also delivered per batch as [`ScanEvent::RepoErrors`] while the scan runs; this is the
    /// complete list for a frontend that missed one, or reloaded mid-scan.
    pub errors: Vec<ScanError>,
    /// Wall-clock duration of the whole scan, milliseconds: the walk and every tier.
    pub elapsed_ms: u64,
    /// How long the walk took, milliseconds.
    pub discovery_ms: u64,
    /// Time in Tier 0, summed over every batch, milliseconds.
    pub tier0_ms: u64,
    /// Time in Tier 1, summed over every batch, milliseconds.
    pub tier1_ms: u64,
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

// ---------------------------------------------------------------------------------------------
// IPC events
//
// The engine does not use these types. They live here because `vp run types` is scoped to
// `-p repo-scan` (§4.2), so an event type declared in `src-tauri` would be the one part of the
// wire the frontend had to mirror by hand. They stay `gix`-free and Tauri-free like everything
// else in this file: `tauri::ipc::Channel<T>` needs only `T: Serialize`.
// ---------------------------------------------------------------------------------------------

/// Identifies one scan for the lifetime of the process.
///
/// A plain counter rather than a UUID: it never leaves this process, it is only ever compared for
/// equality, and a `u64` costs no dependency. The counter starts at 1, so `0` is never a live scan
/// and a frontend default cannot accidentally match one.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
// Redundant for a newtype, which serde already serializes as its inner value — stated so that
// adding a second field is a compile error here rather than a silent change of the wire from a
// number to an array. `ts-rs` cannot parse this attribute and says so on every `vp run types`;
// the note is expected, the generated `export type ScanId = number` is correct, and neither is
// worth removing the guard for.
#[serde(transparent)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub struct ScanId(pub u64);

impl std::fmt::Display for ScanId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// What a scan reports as it runs. One `Channel<ScanEvent>` per `scan_roots` call.
///
/// Every variant carries its [`ScanId`]: a rescan or a root change mid-scan must not be able to
/// interleave stale rows with fresh ones, and the frontend drops any event whose id is not the one
/// it asked for. Rows are batched rather than sent one per repository — the per-message JSON
/// serialization is the cost that bites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub enum ScanEvent {
    /// Repositories the walk found, before any Git read. Rows paint from these, with every tiered
    /// field rendering as unknown.
    #[serde(rename_all = "camelCase")]
    ReposFound {
        /// The scan that produced this batch.
        scan_id: ScanId,
        /// The batch, at most one flush window's worth.
        repos: Vec<DiscoveredRepo>,
    },

    /// Full merged rows, after Tier 0. Merged in Rust, so the frontend replaces wholesale.
    #[serde(rename_all = "camelCase")]
    ReposUpdated {
        /// The scan that produced this batch.
        scan_id: ScanId,
        /// The batch of merged rows.
        repos: Vec<RepoStatus>,
    },

    /// Running totals, so the frontend renders progress from values Rust holds rather than by
    /// counting the rows it happens to have received.
    #[serde(rename_all = "camelCase")]
    Progress {
        /// The scan being reported on.
        scan_id: ScanId,
        /// Repositories reported by [`ScanEvent::ReposFound`] so far.
        found: u32,
        /// Rows merged and reported by [`ScanEvent::ReposUpdated`] so far.
        read: u32,
    },

    /// The walk is over. Carries the final repository count, which is the denominator a progress
    /// bar needs.
    ///
    /// Emitted the moment the walk returns, so it arrives early enough to be useful. It therefore
    /// says nothing about the row events: batches for repositories counted here may still be in
    /// flight behind it.
    #[serde(rename_all = "camelCase")]
    DiscoveryFinished {
        /// The scan whose walk finished.
        scan_id: ScanId,
        /// What the walk did — counts, pruned directories, and non-fatal path failures.
        summary: ScanSummary,
    },

    /// Repositories in the batch just read that produced **no row at all**, with the cause.
    ///
    /// The *total* failure grade, delivered as it happens. It arrives per batch rather than only in
    /// the terminal event because a scan is not quick: at Tier 1's cold cost a three-hundred
    /// repository tree runs for the better part of a minute, and a row whose HEAD could not be read
    /// would otherwise render as "counting…" — a claim that work is in progress — for all of it.
    ///
    /// A repository that *was* read but lost one field is not here: it has a row, and its cause
    /// rides on [`RepoStatus::error`].
    #[serde(rename_all = "camelCase")]
    RepoErrors {
        /// The scan that produced these failures.
        scan_id: ScanId,
        /// The failures, by path.
        errors: Vec<ScanError>,
    },

    /// The scan ran to completion. Terminal.
    ///
    /// `summary.errors` is the complete list of *total* per-repository failures — the same ones
    /// [`ScanEvent::RepoErrors`] already delivered per batch, repeated here so a frontend that
    /// reloaded mid-scan is not left without them.
    #[serde(rename_all = "camelCase")]
    Finished {
        /// The scan that finished.
        scan_id: ScanId,
        /// What the whole scan did, tier by tier.
        summary: ScanTotals,
    },

    /// The scan stopped before completing. Terminal.
    ///
    /// Covers `cancel_scan`, a root change that superseded it, window close, and an internal
    /// failure that unwound the pipeline. The frontend treats all four alike: stop the spinner,
    /// keep the rows already delivered, show the counts as final. There is deliberately no
    /// separate "failed" variant — from the row state's point of view the outcomes are identical,
    /// and a variant meaning "the same as cancelled, but sadder" earns nothing.
    #[serde(rename_all = "camelCase")]
    Cancelled {
        /// The scan that stopped.
        scan_id: ScanId,
        /// Repositories reported found before it stopped.
        found: u32,
        /// Rows merged before it stopped.
        read: u32,
    },
}

/// A row change that is not part of a scan.
///
/// One `Channel<RepoEvent>` per app session, opened by `subscribe` at startup and held in state for
/// the process's life. This is the channel the watcher, the poll, and fetch completions push over:
/// each refreshes in Rust and sends the merged row here. Scan results go on the scan's own channel
/// instead, so a scan's ordering and a session push can never be confused for one another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]
pub enum RepoEvent {
    /// Full merged rows, refreshed outside a scan. Batched like scan results, for the same reason.
    #[serde(rename_all = "camelCase")]
    Updated {
        /// The merged rows.
        repos: Vec<RepoStatus>,
    },

    /// Rows that are no longer in the canonical map, by path. The mirror must drop them.
    #[serde(rename_all = "camelCase")]
    Removed {
        /// Absolute paths, exactly as Rust spells them.
        paths: Vec<PathBuf>,
    },
}
