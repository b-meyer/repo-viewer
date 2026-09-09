//! Tier 2: the full file counts and the submodule list.
//!
//! Tier 1 answers "is there anything uncommitted?" by abandoning the status iterator after its
//! first item. Tier 2 answers "what, and how much?", which means draining that iterator to the
//! end — roughly the same work again with none of the early exit. That is why it is lazy, and why
//! nothing in `src-tauri/src/pipeline.rs` calls it: it runs when a row is expanded or refreshed,
//! one repository at a time.
//!
//! There is deliberately **no `read_tier2_all_with`**. Every other tier has a rayon fan-out because
//! every other tier runs over a whole tree; a fan-out here would be an invitation to put Tier 2
//! into the scan path, which is the one thing the tiering exists to prevent.
//!
//! # The counts are per-column, not a partition of paths
//!
//! `into_iter` runs two comparisons at once — HEAD's tree against the index, and the index against
//! the worktree — and **both can emit for the same path**. A file that was staged and then modified
//! again is one `TreeIndex` item and one `IndexWorktree` item, which is exactly what `git status`
//! renders as the two-letter code `MM`. So [`FileCounts`] holds four independent column totals, not
//! four slices of one set: they must never be summed, and nothing may present them as a number of
//! changed files.
//!
//! # Three traps in the classification
//!
//! **`EntryStatus::NeedsUpdate` never reaches this code**, and that is worth knowing rather than
//! guarding against. It means "the entry did not change, but checking it was expensive"; `gix`'s
//! own iterator diverts it into the index writeback list and yields nothing, so it can neither
//! inflate a count here nor falsely trip Tier 1's early exit.
//!
//! **`EntryStatus::IntentToAdd` does reach it**, and is the trap in its place. `git add -N` records
//! an index entry promising content the object database does not hold, and git counts that as
//! **unstaged only** — `git status --porcelain` prints ` A`, with the index column empty. Reading
//! the `A` as a staged addition is the mistake, and an easy one, because the entry genuinely is in
//! the index. `gix` agrees with git here and emits no tree-index change for it, so the count is
//! right only if this arm does not add one either.
//!
//! **Rename tracking is on by default for the tree-index half.** `TrackRenames::AsConfigured`
//! falls back to enabled when neither `status.renames` nor `diff.renames` is set, so a staged
//! rename arrives as a single `Rewrite` change spanning two paths — one item, matching the one
//! `R old -> new` line porcelain prints. Counting its two paths separately would double it.

use std::{
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use gix::bstr::{BString, ByteSlice};

use crate::{
    error::{Error, Result},
    model::{DiscoveredRepo, FileCounts, RepoKind, SubmoduleStatus},
};

/// What Tier 2 read for one repository.
///
/// Not a wire type and not a partial [`RepoStatus`](crate::model::RepoStatus): the glue layer
/// merges it into the row the earlier tiers produced and sends the merged result, so the frontend
/// never reconciles two tiers itself.
///
/// Both fields are `Option` because the two reads are independent and either can fail on its own —
/// the status iterator touches the worktree, the submodule list reads `.gitmodules` and possibly
/// the whole index. `None` on one of them means that half failed and [`Tier2::error`] says why; it
/// does not mean the value is zero or empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier2 {
    /// The repository this describes. The canonical map's key.
    pub path: PathBuf,
    /// The four column totals. `None` when the status read failed.
    pub counts: Option<FileCounts>,
    /// The submodules this repository records. `None` when the enumeration failed; `Some(vec![])`
    /// when it succeeded and there are none.
    pub submodules: Option<Vec<SubmoduleStatus>>,
    /// A partial failure. The row survives; this says which half of it is missing.
    pub error: Option<String>,
}

/// Read Tier 2 for one repository.
///
/// `Ok(None)` means the repository has no worktree, so neither question applies — a bare repository
/// has nothing to compare against and no `.gitmodules` to read. That is "cannot ever" rather than
/// "not yet", and leaving the row's fields `None` is what lets the UI say so.
///
/// `Err` means the repository could not be opened at all. Anything narrower is a value on
/// [`Tier2::error`].
///
/// `should_interrupt` is handed to `gix` for use inside the walk, which is why this takes an `Arc`
/// where Tier 0 takes a plain reference: `gix`'s interrupt hook accepts an owned `Arc` or a
/// `&'static` flag, and nothing here is `'static`.
pub fn read_tier2(
    found: &DiscoveredRepo,
    should_interrupt: &Arc<AtomicBool>,
) -> Result<Option<Tier2>> {
    if found.kind == RepoKind::Bare {
        return Ok(None);
    }

    let git_dir = found.git_dir.as_path();
    let repo = guarded(git_dir, || open_repo(found))?;

    let mut failures: Vec<String> = Vec::new();

    let counts = optional(git_dir, &mut failures, || {
        count_changes(&repo, should_interrupt)
    });
    let submodules = optional(git_dir, &mut failures, || read_submodules(&repo));

    Ok(Some(Tier2 {
        path: found.path.clone(),
        counts,
        submodules,
        error: (!failures.is_empty()).then(|| failures.join("; ")),
    }))
}

/// Open the repository at the Git directory discovery already resolved.
///
/// `open_path_as_is` for the same reason the other tiers use it: without it, `open_opts` joins
/// `.git` onto the path and re-runs the classification the walk already did.
fn open_repo(found: &DiscoveredRepo) -> Result<gix::Repository> {
    let options = gix::open::Options::default().open_path_as_is(true);
    gix::open_opts(&found.git_dir, options).map_err(|err| Error::OpenRepo {
        path: found.git_dir.clone(),
        message: err.to_string(),
    })
}

/// Drain the status iterator, tallying one column per item.
///
/// `into_iter` rather than `into_index_worktree_iter`, so the HEAD-tree comparison stays active and
/// staged changes are seen at all — the trap `tier1.rs` documents. `Collapsed` untracked files
/// because that is what `git status` reports by default: one untracked directory is one entry, not
/// one per file inside it.
fn count_changes(repo: &gix::Repository, should_interrupt: &Arc<AtomicBool>) -> Result<FileCounts> {
    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|err| worktree_error(repo, "status", &err))?
        .untracked_files(gix::status::UntrackedFiles::Collapsed)
        // A private flag, never the shared one — see `private_interrupt`. Tier 2 drains the
        // iterator rather than exiting early, but the iterator's `Drop` sets the flag regardless,
        // and this one is reached from a command that may run while a scan is in flight.
        .should_interrupt_owned(crate::status::private_interrupt(should_interrupt))
        .into_iter(Vec::<BString>::new())
        .map_err(|err| worktree_error(repo, "status iterator", &err))?;

    let mut counts = FileCounts::default();
    for item in iter {
        let item = item.map_err(|err| worktree_error(repo, "status", &err))?;
        tally(&mut counts, &item);
    }
    Ok(counts)
}

/// Add one status item to the column it belongs in.
///
/// **No wildcard arm, at either level.** A new `gix` variant must be a compile error here rather
/// than a silently uncounted change — the same discipline `tier0.rs` applies to
/// `gix::state::InProgress`, and for the same reason: a count that quietly omits a kind of change
/// is the uncomputed-renders-as-zero bug with extra steps.
fn tally(counts: &mut FileCounts, item: &gix::status::Item) {
    use gix::status::{Item, index_worktree, plumbing::index_as_worktree::EntryStatus};

    match item {
        // HEAD's tree against the index: staged work, whatever its shape. A `Rewrite` change is
        // one item spanning two paths and stays one, matching porcelain's single `R` line.
        Item::TreeIndex(_) => counts.staged = counts.staged.saturating_add(1),

        Item::IndexWorktree(change) => match change {
            index_worktree::Item::Modification { status, .. } => match status {
                // Stages 1-3 of one path arrive as a single item here, so this is already a count
                // of files rather than of index entries.
                EntryStatus::Conflict { .. } => {
                    counts.conflicted = counts.conflicted.saturating_add(1);
                }
                EntryStatus::Change(_) => counts.unstaged = counts.unstaged.saturating_add(1),
                // `git add -N`: the index promises content the object database does not have. git
                // counts this as unstaged alone — porcelain prints ` A`, index column empty — and
                // emits no tree-index change for it, so nothing adds to `staged` here either.
                EntryStatus::IntentToAdd => counts.unstaged = counts.unstaged.saturating_add(1),
                // Diverted into the iterator's index-writeback list before it reaches a consumer,
                // so this arm exists only to keep the match exhaustive. Counting it would report
                // an unchanged file as modified.
                EntryStatus::NeedsUpdate(_) => {}
            },

            // The directory walk. Ignored entries are not emitted at this configuration, but the
            // status is checked rather than assumed: `Tracked` and `Pruned` entries reaching here
            // are not changes, and an `Ignored` one would never be.
            index_worktree::Item::DirectoryContents { entry, .. } => {
                if entry.status == gix::dir::entry::Status::Untracked {
                    counts.untracked = counts.untracked.saturating_add(1);
                }
            }

            // Unreachable at this configuration — index-worktree rename tracking is off unless
            // `rewrites` is set, which nothing here sets. Mapped rather than ignored so enabling
            // it later cannot silently drop the change.
            index_worktree::Item::Rewrite { .. } => {
                counts.unstaged = counts.unstaged.saturating_add(1);
            }
        },
    }
}

/// The submodules this repository records, read from its own configuration.
///
/// Never from the walk: `.gitmodules` is the parent's own account of its submodules, so the list
/// and the rows can never disagree about what exists.
///
/// `submodules()` yielding `Ok(None)` means there is **no submodule configuration at all**, which
/// is an answered question and returns an empty `Vec`. Reporting it as unknown would claim a read
/// is still outstanding for the overwhelmingly common case.
///
/// Everything here happens inside this function on purpose: a `gix::Submodule` holds an `Rc` and so
/// is `!Send`, and cannot be carried out to a caller on another thread.
fn read_submodules(repo: &gix::Repository) -> Result<Vec<SubmoduleStatus>> {
    let Some(modules) = repo
        .submodules()
        .map_err(|err| worktree_error(repo, "submodules", &err))?
    else {
        return Ok(Vec::new());
    };

    let mut listed = Vec::new();
    for module in modules {
        let name = module.name().to_string();
        let path = module
            .path()
            .map_err(|err| worktree_error(repo, "submodule path", &err))?;

        // The recorded id comes from the index; the checked-out one means opening the submodule's
        // own repository, and is `None` when it has never been initialised.
        let recorded_id = module
            .index_id()
            .map_err(|err| worktree_error(repo, "submodule index id", &err))?;
        let head_id = module
            .head_id()
            .map_err(|err| worktree_error(repo, "submodule head id", &err))?;

        listed.push(SubmoduleStatus {
            name,
            path: PathBuf::from(path.to_path_lossy().as_ref()),
            recorded_id: recorded_id.map(|id| id.to_hex().to_string()),
            head_id: head_id.map(|id| id.to_hex().to_string()),
        });
    }

    // `names()` yields configuration order, which is `.gitmodules` order. Sorted so a row's list is
    // stable between reads regardless of how that file is edited.
    listed.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(listed)
}

/// A worktree-level failure, named so the message says which read gave up.
fn worktree_error(
    repo: &gix::Repository,
    what: &'static str,
    err: &dyn std::fmt::Display,
) -> Error {
    Error::Worktree {
        path: repo.git_dir().to_path_buf(),
        what,
        message: err.to_string(),
    }
}

/// Run a read, turning a caught panic into [`Error::Panicked`].
///
/// `AssertUnwindSafe` for the same reason the other tiers need it: `&gix::Repository` holds interior
/// mutability for its object caches. Sound because a panicking read is abandoned entirely.
fn guarded<T>(path: &Path, read: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(AssertUnwindSafe(read)) {
        Ok(result) => result,
        Err(payload) => Err(Error::Panicked {
            path: path.to_path_buf(),
            message: panic_message(&*payload),
        }),
    }
}

/// Run one half of the read: on failure, record the cause and yield `None`.
fn optional<T>(
    path: &Path,
    failures: &mut Vec<String>,
    read: impl FnOnce() -> Result<T>,
) -> Option<T> {
    match guarded(path, read) {
        Ok(value) => Some(value),
        Err(err) => {
            failures.push(err.to_string());
            None
        }
    }
}

/// The payload of a caught panic, when it was a string.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panicked".to_string())
}
