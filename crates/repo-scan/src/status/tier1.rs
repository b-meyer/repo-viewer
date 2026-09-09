//! Tier 1: the dirty flag and the conflicted count.
//!
//! Tier 0 answers "what have I not pushed?" from refs alone. Tier 1 answers "what have I not
//! committed?", which needs the worktree — but only barely: the status iterator is abandoned after
//! its **first** item, because the question is whether there is *any* change, not what the changes
//! are. Counting them is Tier 2, and is lazy for that reason.
//!
//! # Three traps live here
//!
//! **`gix::Repository::is_dirty()` is not the function to use.** Its own documentation says
//! "*untracked files* do *not* affect this flag", and it disables the directory walk internally. A
//! repository whose only change is a new file reports clean.
//!
//! **`into_index_worktree_iter()` is not the iterator to use either**, which is less obvious. It
//! sets `head_tree = None` and so compares only the index against the worktree — a repository with
//! staged-but-uncommitted changes and a clean worktree reports clean through it. `into_iter()`
//! keeps the HEAD-tree comparison that `status()` sets up by default and runs all three checks at
//! once: the directory walk for untracked files, index-against-worktree for unstaged changes, and
//! tree-against-index for staged ones. Any one of those yielding an item means dirty.
//!
//! **A conflicted path has up to three index entries, not one.** A merge conflict writes stages 1,
//! 2 and 3 for the same path, so counting entries with a non-zero stage reports three times the
//! number of conflicted files. The count here is of distinct paths, which is what `git status`
//! reports and what a reader will compare it against.

use std::{
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use gix::bstr::BString;
use rayon::prelude::*;

use crate::{
    error::{Error, Result},
    model::{DiscoveredRepo, RepoKind, ScanError},
};

/// What Tier 1 read for one repository.
///
/// Not a partial [`RepoStatus`](crate::model::RepoStatus) and not a wire type: it never crosses
/// IPC on its own. The glue layer merges it into the row Tier 0 produced and sends the merged
/// result, so that the frontend never has to reconcile two tiers itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1 {
    /// The repository this describes. The canonical map's key.
    pub path: PathBuf,
    /// Whether the worktree differs from HEAD, **including untracked files**.
    pub dirty: bool,
    /// Distinct paths with a conflicted index entry.
    pub conflicted: u32,
    /// A partial failure. The row survives; this says which half of it is missing.
    pub error: Option<String>,
}

/// What a completed Tier 1 pass did, as opposed to what it read.
///
/// Shaped like [`Tier0Summary`](crate::model::Tier0Summary) deliberately. The errors here are
/// repositories whose worktree could not be examined at all; one that was examined but whose
/// conflicted count failed carries its message on [`Tier1::error`] and is counted in `repos_read`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tier1Summary {
    /// Repositories that produced a result.
    pub repos_read: u32,
    /// Repositories skipped because they have no worktree to examine.
    pub bare_skipped: u32,
    /// Repositories whose worktree could not be examined. Never fatal.
    pub errors: Vec<ScanError>,
    /// Wall-clock duration of the pass, milliseconds.
    pub elapsed_ms: u64,
}

/// Read Tier 1 for every repository in `repos`, calling `on_status` once per result.
///
/// `on_status` runs on a rayon worker and is called concurrently, so it must be cheap.
///
/// `should_interrupt` is checked between repositories **and** handed to `gix` for use inside the
/// walk, which is why this takes an `Arc` where Tier 0 takes a plain reference: `gix`'s own
/// interrupt hook accepts an owned `Arc` or a `&'static` flag, and nothing here is `'static`.
///
/// Bare repositories are skipped rather than reported. They have no worktree, so their `dirty` and
/// `conflicted` are not "not yet computed" but "cannot ever be" — leaving them `None` on the row is
/// what lets the UI say so.
pub fn read_tier1_all_with<F>(
    repos: &[DiscoveredRepo],
    should_interrupt: &Arc<AtomicBool>,
    on_status: F,
) -> Tier1Summary
where
    F: Fn(Tier1) + Send + Sync,
{
    let started = Instant::now();
    let errors = std::sync::Mutex::new(Vec::new());
    let read = std::sync::atomic::AtomicU32::new(0);
    let skipped = std::sync::atomic::AtomicU32::new(0);

    repos.par_iter().for_each(|found| {
        if should_interrupt.load(Ordering::Relaxed) {
            return;
        }
        match read_tier1(found, should_interrupt) {
            Ok(Some(status)) => {
                read.fetch_add(1, Ordering::Relaxed);
                on_status(status);
            }
            Ok(None) => {
                skipped.fetch_add(1, Ordering::Relaxed);
            }
            Err(err) => lock(&errors).push(ScanError {
                path: found.path.clone(),
                message: err.to_string(),
            }),
        }
    });

    Tier1Summary {
        repos_read: read.load(Ordering::Relaxed),
        bare_skipped: skipped.load(Ordering::Relaxed),
        errors: errors
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        elapsed_ms: elapsed_ms(started),
    }
}

/// Read Tier 1 for one repository.
///
/// `Ok(None)` means the repository has no worktree, so the question does not apply. `Err` means the
/// worktree could not be examined at all.
pub fn read_tier1(
    found: &DiscoveredRepo,
    should_interrupt: &Arc<AtomicBool>,
) -> Result<Option<Tier1>> {
    if found.kind == RepoKind::Bare {
        return Ok(None);
    }

    let git_dir = found.git_dir.as_path();
    let repo = guarded(git_dir, || open_repo(found))?;

    let mut failures: Vec<String> = Vec::new();

    // The index read comes first: it is the cheaper of the two and does no worktree I/O at all.
    let conflicted = match guarded(git_dir, || count_conflicted(&repo)) {
        Ok(count) => count,
        Err(err) => {
            failures.push(err.to_string());
            0
        }
    };

    let dirty = guarded(git_dir, || is_dirty(&repo, should_interrupt))?;

    Ok(Some(Tier1 {
        path: found.path.clone(),
        dirty,
        conflicted,
        error: (!failures.is_empty()).then(|| failures.join("; ")),
    }))
}

/// Open the repository at the Git directory discovery already resolved.
///
/// `open_path_as_is` for the same reason Tier 0 uses it: without it, `open_opts` joins `.git` onto
/// the path and re-runs the classification the walk already did.
fn open_repo(found: &DiscoveredRepo) -> Result<gix::Repository> {
    let options = gix::open::Options::default().open_path_as_is(true);
    gix::open_opts(&found.git_dir, options).map_err(|err| Error::OpenRepo {
        path: found.git_dir.clone(),
        message: err.to_string(),
    })
}

/// Whether anything differs from HEAD, untracked files included.
///
/// Early-exits on the first item. `into_iter` rather than `into_index_worktree_iter` so the
/// HEAD-tree comparison stays active and a staged-only change still counts — see the module docs.
fn is_dirty(repo: &gix::Repository, should_interrupt: &Arc<AtomicBool>) -> Result<bool> {
    let mut iter = repo
        .status(gix::progress::Discard)
        .map_err(|err| worktree_error(repo, "status", &err))?
        .untracked_files(gix::status::UntrackedFiles::Collapsed)
        .should_interrupt_owned(Arc::clone(should_interrupt))
        .into_iter(Vec::<BString>::new())
        .map_err(|err| worktree_error(repo, "status iterator", &err))?;

    match iter.next() {
        // Any item at all — a staged change, an unstaged one, or an untracked file — is a change.
        Some(Ok(_)) => Ok(true),
        Some(Err(err)) => Err(worktree_error(repo, "status", &err)),
        None => Ok(false),
    }
}

/// How many distinct paths have a conflicted index entry.
///
/// Reads `.git/index` only, so this is not worktree I/O. A conflict writes up to three entries for
/// one path (stages 1, 2 and 3); the index is sorted by path, so counting the transitions gives
/// the number of files without allocating a set.
fn count_conflicted(repo: &gix::Repository) -> Result<u32> {
    let index = match repo.index() {
        Ok(index) => index,
        // No index at all is not a failure: a freshly initialised repository has none, and it
        // plainly has no conflicts either.
        Err(gix::worktree::open_index::Error::IndexFile(_)) => return Ok(0),
        Err(err) => return Err(worktree_error(repo, "index", &err)),
    };

    let mut conflicted = 0u32;
    let mut previous: Option<&gix::bstr::BStr> = None;
    for entry in index.entries() {
        if entry.stage_raw() == 0 {
            continue;
        }
        let path = entry.path(&index);
        if previous.is_none_or(|last| last != path) {
            conflicted = conflicted.saturating_add(1);
        }
        previous = Some(path);
    }
    Ok(conflicted)
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
/// `AssertUnwindSafe` for the same reason Tier 0 needs it: `&gix::Repository` holds interior
/// mutability for its object caches and so is not `UnwindSafe`. Sound here because a panicking read
/// is abandoned entirely.
fn guarded<T>(path: &Path, read: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(AssertUnwindSafe(read)) {
        Ok(result) => result,
        Err(payload) => Err(Error::Panicked {
            path: path.to_path_buf(),
            message: panic_message(&*payload),
        }),
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

/// Lock through a poison rather than cascading one worker's panic into every other.
fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Milliseconds since `started`, saturating.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
