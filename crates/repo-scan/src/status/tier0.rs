//! Tier 0: everything a row needs that can be had from refs alone.
//!
//! Branch, upstream, ahead/behind, stash count, in-progress state, tip commit, last-fetched age.
//! No worktree is touched and no index is read, which is what makes a row appear before anything
//! expensive happens to it. **This must stay refs-only** — the dirty flag and the file counts are
//! Tiers 1 and 2 and belong on the lazy path.
//!
//! The submodule list is deliberately absent, and it is the one field whose tier is not obvious.
//! `gix`'s `Repository::submodules()` reads `.gitmodules` from the *worktree*, and when that file
//! is missing it falls back to parsing the whole index and then the HEAD tree — so even the names
//! and paths alone are not a refs read. It belongs to Tier 2 entirely.
//!
//! # Two grades of failure
//!
//! A repository that cannot be opened, or whose HEAD cannot be read, produces **no row**. Every
//! other Tier 0 field is measured relative to HEAD, and [`RepoStatus`]'s Tier 0 fields are not
//! `Option`, so a row without a head would have to invent one. Those failures come back as `Err`
//! and land in [`Tier0Summary::errors`].
//!
//! A repository that *was* read but whose upstream, ahead/behind, or stash count failed keeps its
//! row: the affected fields stay `None` and the cause is reported on [`RepoStatus::error`].
//! Losing one field is not worth losing the row.
//!
//! # Panics are values too
//!
//! `gix` can panic on a corrupt object or pack. Every read here runs under `catch_unwind`, at a
//! granularity that matches the two grades above: a panic while opening or reading HEAD is total,
//! a panic in any later field is partial. Because the catch is per-field and inside this function,
//! rayon never observes a panic and its worker-panic propagation never fires — which is why the
//! release profile keeps `panic = "unwind"`.

use std::{
    panic::AssertUnwindSafe,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use gix::{ObjectId, refs::FullName, remote::Direction};
use rayon::prelude::*;

use crate::{
    error::{Error, Result},
    model::{CommitSummary, DiscoveredRepo, Head, RepoState, RepoStatus, ScanError, Tier0Summary},
    status::ahead_behind::{AHEAD_BEHIND_CAP, ahead_behind},
};

/// Read Tier 0 for every repository in `repos`, collecting the rows.
///
/// Rows come back sorted by path: the fan-out completes in a nondeterministic order and a caller
/// that wants a stable list should not have to sort it. Use [`read_tier0_all_with`] when rows
/// should stream as they are read.
pub fn read_tier0_all(
    repos: &[DiscoveredRepo],
    should_interrupt: &AtomicBool,
) -> (Vec<RepoStatus>, Tier0Summary) {
    let read = std::sync::Mutex::new(Vec::new());
    let summary = read_tier0_all_with(repos, should_interrupt, |status| {
        lock(&read).push(status);
    });

    let mut rows = read
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rows.sort_by(|left, right| left.path.cmp(&right.path));
    (rows, summary)
}

/// Read Tier 0 for every repository in `repos`, calling `on_status` once per row as it is read.
///
/// `on_status` runs on a rayon worker and is called concurrently, so it must be cheap: anything
/// slow done here stalls the fan-out. The IPC layer batches these into channel sends rather than
/// sending one per repository.
///
/// `should_interrupt` is checked between repositories. A cancelled pass returns the rows it had
/// already read — it does not discard them, and it is not an error.
pub fn read_tier0_all_with<F>(
    repos: &[DiscoveredRepo],
    should_interrupt: &AtomicBool,
    on_status: F,
) -> Tier0Summary
where
    F: Fn(RepoStatus) + Send + Sync,
{
    let started = Instant::now();
    let errors = std::sync::Mutex::new(Vec::new());
    let read = std::sync::atomic::AtomicU32::new(0);

    repos.par_iter().for_each(|found| {
        if should_interrupt.load(Ordering::Relaxed) {
            return;
        }
        match read_tier0(found) {
            Ok(status) => {
                read.fetch_add(1, Ordering::Relaxed);
                on_status(status);
            }
            Err(err) => lock(&errors).push(ScanError {
                path: found.path.clone(),
                message: err.to_string(),
            }),
        }
    });

    Tier0Summary {
        repos_read: read.load(Ordering::Relaxed),
        errors: errors
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        elapsed_ms: elapsed_ms(started),
    }
}

/// Read Tier 0 for one repository.
///
/// `Err` means no honest row could be produced — see the two grades of failure in the module
/// docs. A returned row may still carry [`RepoStatus::error`] for a field that failed.
pub fn read_tier0(found: &DiscoveredRepo) -> Result<RepoStatus> {
    let git_dir = found.git_dir.as_path();

    // Total-failure stage. Both of these are wrapped, and a panic in either means no row.
    let repo = guarded(git_dir, || open_repo(found))?;
    let head = guarded(git_dir, || read_head(&repo, git_dir))?;

    // Partial stage. Each field group degrades on its own and records why.
    let mut failures: Vec<String> = Vec::new();

    let tracking = optional(git_dir, &mut failures, || match head.branch.as_ref() {
        Some(branch) => upstream_of(&repo, branch),
        None => Ok(None),
    })
    .flatten();

    // `ahead`/`behind` stay `None` unless there is both an upstream *and* a local tip to compare
    // it against — a configured upstream that has never been fetched has no ref to count towards.
    let (ahead, behind) = match (tracking.as_ref(), head.tip) {
        (Some(upstream), Some(tip)) => match upstream.id {
            Some(upstream_id) => optional(git_dir, &mut failures, || {
                ahead_behind(&repo, tip, upstream_id, AHEAD_BEHIND_CAP)
            })
            .map_or((None, None), |(ahead, behind)| (Some(ahead), Some(behind))),
            None => (None, None),
        },
        _ => (None, None),
    };

    let last_commit = match head.tip {
        Some(tip) => optional(git_dir, &mut failures, || read_commit(&repo, tip)),
        None => None,
    };

    // `stash_count` is not `Option` in the model, so a failed read has to report *some* number.
    // It reports 0 and records the failure, which is why the row also carries `error`: the count
    // is only trustworthy on a row without one.
    let stash_count = optional(git_dir, &mut failures, || read_stash_count(&repo)).unwrap_or(0);

    let state =
        optional(git_dir, &mut failures, || Ok(read_state(&repo))).unwrap_or(RepoState::Clean);

    // Both directories, newest wins. See `fetch_head_ms` — neither alone is the answer.
    let last_fetched_ms = fetch_head_ms(repo.git_dir(), repo.common_dir());

    Ok(RepoStatus {
        path: found.path.clone(),
        name: found.name.clone(),
        parent: found.parent.clone(),
        kind: found.kind,

        head: head.head,
        upstream: tracking.map(|tracking| tracking.name),
        ahead,
        behind,
        last_commit,
        stash_count,
        state,
        last_fetched_ms,

        dirty: None,
        conflicted: None,

        counts: None,
        submodules: None,

        scanned_at_ms: now_ms(),
        error: (!failures.is_empty()).then(|| failures.join("; ")),
    })
}

/// Open the repository at the Git directory discovery already resolved.
///
/// `open_path_as_is` matters: without it `open_opts` joins `.git` onto the path and re-runs
/// `gix_discover::is_git`, repeating per repository the classification the walk already did. The
/// ownership-based trust check is deliberately left in place — it is what downgrades config trust
/// for a repository owned by someone else, which is a real case on a share.
fn open_repo(found: &DiscoveredRepo) -> Result<gix::Repository> {
    let options = gix::open::Options::default().open_path_as_is(true);
    gix::open_opts(&found.git_dir, options).map_err(|err| Error::OpenRepo {
        path: found.git_dir.clone(),
        message: err.to_string(),
    })
}

/// What HEAD yielded: the wire value, the tip to count from, and the branch to resolve upstream.
struct HeadInfo {
    /// The wire representation.
    head: Head,
    /// The commit HEAD resolves to, fully peeled. `None` for an unborn branch.
    tip: Option<ObjectId>,
    /// The branch HEAD is on, for the upstream lookup. `None` when detached or unborn.
    branch: Option<FullName>,
}

/// Resolve HEAD.
///
/// `try_peel_to_id`, never `Head::id()`. `id()` resolves a detached HEAD as
/// `peeled.unwrap_or(target)`, and `peeled` is only ever populated from a `packed-refs` `^` line —
/// a loose `.git/HEAD` holding a bare object id gives `peeled: None`. So on a HEAD detached onto an
/// annotated tag, `id()` hands back the *tag's* id and calls it a commit. `try_peel_to_id` reads
/// the object header and peels tags to their end, which costs nothing extra on the common
/// symbolic case.
fn read_head(repo: &gix::Repository, git_dir: &Path) -> Result<HeadInfo> {
    let mut head = repo.head().map_err(|err| Error::ReadHead {
        path: git_dir.to_path_buf(),
        message: err.to_string(),
    })?;

    let tip = head
        .try_peel_to_id()
        .map_err(|err| Error::ReadHead {
            path: git_dir.to_path_buf(),
            message: err.to_string(),
        })?
        .map(|id| id.detach());
    let branch = head.referent_name().map(|name| name.to_owned());

    let wire = match (&head.kind, tip) {
        (gix::head::Kind::Unborn(_), _) => Head::Unborn,
        (gix::head::Kind::Symbolic(_), _) => Head::Branch {
            // `shorten` turns `refs/heads/feature/x` into `feature/x`.
            name: branch
                .as_ref()
                .map(|name| name.shorten().to_string())
                .unwrap_or_default(),
        },
        (gix::head::Kind::Detached { .. }, Some(id)) => Head::Detached {
            id: id.to_hex().to_string(),
        },
        // A detached HEAD always resolves to an id; this arm cannot be reached in practice, and
        // reporting it as unborn is closer to the truth than inventing a commit.
        (gix::head::Kind::Detached { .. }, None) => Head::Unborn,
    };

    Ok(HeadInfo {
        head: wire,
        tip,
        branch,
    })
}

/// The upstream tracking branch of a local branch.
struct Tracking {
    /// Display name, e.g. `origin/main`.
    name: String,
    /// The commit the tracking ref points at, or `None` when it has never been fetched.
    id: Option<ObjectId>,
}

/// Resolve the upstream of `branch`, if one is configured.
///
/// A configured upstream with no local tracking ref yet — a branch pushed but never fetched, or a
/// fresh clone of a branch that does not exist on the remote — returns the name with no id. That
/// is deliberately distinct from "no upstream": the name is shown, and ahead/behind stay unknown
/// rather than reading as zero.
fn upstream_of(repo: &gix::Repository, branch: &FullName) -> Result<Option<Tracking>> {
    let Some(tracking) = repo.branch_remote_tracking_ref_name(branch.as_ref(), Direction::Fetch)
    else {
        return Ok(None);
    };
    let tracking = tracking.map_err(|err| refs_error(repo, "upstream", err))?;

    let id = repo
        .try_find_reference(tracking.as_ref())
        .map_err(|err| refs_error(repo, "upstream", err))?
        .map(|reference| {
            reference
                .into_fully_peeled_id()
                .map_err(|err| refs_error(repo, "upstream", err))
                .map(|id| id.detach())
        })
        .transpose()?;

    Ok(Some(Tracking {
        name: tracking.shorten().to_string(),
        id,
    }))
}

/// Summarise the tip commit.
///
/// Author time, not committer time: `Commit::time()` is the committer's, and a rebase rewrites it
/// while leaving authorship alone — so the committer's time would make an old commit look new.
fn read_commit(repo: &gix::Repository, tip: ObjectId) -> Result<CommitSummary> {
    let commit = repo
        .find_commit(tip)
        .map_err(|err| refs_error(repo, "tip commit", err))?;

    let summary = commit
        .message()
        .map_err(|err| refs_error(repo, "tip commit", err))?
        .summary()
        .to_string();

    let author = commit
        .author()
        .map_err(|err| refs_error(repo, "tip commit", err))?;
    let time = author
        .time()
        .map_err(|err| refs_error(repo, "tip commit", err))?;

    Ok(CommitSummary {
        id: tip.to_hex().to_string(),
        summary,
        author: author.name.to_string(),
        time_ms: epoch_ms(time.seconds),
    })
}

/// Count stash entries.
///
/// `gix` 0.87 has no stash API at all, so this reads what `git stash list` reads: the reflog of
/// `refs/stash`, one line per entry. A repository that has never stashed has no such ref, which is
/// a count of zero rather than a failure.
fn read_stash_count(repo: &gix::Repository) -> Result<u32> {
    let Some(stash) = repo
        .try_find_reference("refs/stash")
        .map_err(|err| refs_error(repo, "stash count", err))?
    else {
        return Ok(0);
    };

    let mut platform = stash.log_iter();
    let Some(entries) = platform
        .all()
        .map_err(|err| refs_error(repo, "stash count", err))?
    else {
        return Ok(0);
    };

    let mut count = 0_u32;
    for entry in entries {
        entry.map_err(|err| refs_error(repo, "stash count", err))?;
        count += 1;
    }
    Ok(count)
}

/// Map `gix`'s in-progress operation onto the wire enum.
///
/// `gix` distinguishes ten; the dashboard shows five. A mailbox application and an interactive
/// rebase are both "rebasing" to someone deciding whether a repository is safe to touch, and the
/// sequence variants differ from their single-commit form only in how many commits remain.
/// Nothing folds into `Clean`.
fn read_state(repo: &gix::Repository) -> RepoState {
    use gix::state::InProgress;

    match repo.state() {
        None => RepoState::Clean,
        Some(InProgress::Merge) => RepoState::Merging,
        Some(InProgress::Bisect) => RepoState::Bisecting,
        Some(InProgress::CherryPick | InProgress::CherryPickSequence) => RepoState::CherryPicking,
        Some(InProgress::Revert | InProgress::RevertSequence) => RepoState::Reverting,
        Some(
            InProgress::Rebase
            | InProgress::RebaseInteractive
            | InProgress::ApplyMailbox
            | InProgress::ApplyMailboxRebase,
        ) => RepoState::Rebasing,
    }
}

/// Modification time of the newest `FETCH_HEAD`, epoch milliseconds.
///
/// `None` when the repository has never been fetched. Ahead/behind is measured against
/// `refs/remotes/*`, so this is the age of that measurement and must be shown beside it.
///
/// # Both directories, and neither one alone is right
///
/// `git fetch` writes `FETCH_HEAD` into the git directory of **whichever worktree ran it**, while
/// the `refs/remotes/*` it updates are shared. For a normal repository the two directories are the
/// same path and the distinction does not exist. For a linked worktree it does, and it cuts both
/// ways: a fetch run in the parent leaves one in the common directory and none in the worktree's
/// private directory, and a fetch run in the worktree leaves one in the private directory and
/// none in the common one. Either file can therefore be the most recent evidence, and reading
/// only one reports "never fetched" for a repository that was fetched seconds ago — in the field
/// whose entire job is to say how stale the counts beside it are.
///
/// Measured on git 2.54.0.windows.1, in both directions.
///
/// `pub(crate)` for [`crate::fetch`], whose repeat-fetch guard asks the same question of the same
/// files, and must get the same answer.
pub(crate) fn fetch_head_ms(git_dir: &Path, common_dir: &Path) -> Option<u64> {
    let one = read_fetch_head_ms(git_dir);
    if git_dir == common_dir {
        return one;
    }
    let other = read_fetch_head_ms(common_dir);

    match (one, other) {
        (Some(one), Some(other)) => Some(one.max(other)),
        (found, None) | (None, found) => found,
    }
}

/// Modification time of one directory's `FETCH_HEAD`.
fn read_fetch_head_ms(dir: &Path) -> Option<u64> {
    let modified = std::fs::metadata(dir.join("FETCH_HEAD"))
        .ok()?
        .modified()
        .ok()?;
    Some(system_time_ms(modified))
}

/// Run a read, turning a caught panic into [`Error::Panicked`].
///
/// `AssertUnwindSafe` because `&gix::Repository` is not `UnwindSafe`: it holds interior mutability
/// for its object caches. The assertion is sound here because a panicking read is abandoned
/// entirely — nothing observes the repository afterwards.
fn guarded<T>(path: &Path, read: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(AssertUnwindSafe(read)) {
        Ok(result) => result,
        Err(payload) => Err(Error::Panicked {
            path: path.to_path_buf(),
            message: panic_message(&*payload),
        }),
    }
}

/// Run a partial read: on failure, record the cause and yield `None`.
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

/// Recover a panic payload's message.
///
/// `panic!("literal")` yields a `&'static str` and `panic!("{x}")` a `String`; those two cover
/// every panic the standard macro produces. `panic_any` with anything else falls through to the
/// placeholder, which is still better than losing the row.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "panic with a non-string payload".to_string()
}

/// Wrap a `gix` failure as [`Error::Refs`].
fn refs_error(repo: &gix::Repository, what: &'static str, err: impl std::fmt::Display) -> Error {
    Error::Refs {
        path: repo.git_dir().to_path_buf(),
        what,
        message: err.to_string(),
    }
}

/// Git's signed epoch seconds as unsigned epoch milliseconds.
///
/// Commit times come from whatever clock wrote them, so a pre-1970 date is not hypothetical. It
/// saturates to the epoch rather than wrapping to the year 584 million.
fn epoch_ms(seconds: i64) -> u64 {
    u64::try_from(seconds).unwrap_or(0).saturating_mul(1_000)
}

/// A `SystemTime` as epoch milliseconds, saturating for anything before the epoch.
fn system_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Now, as epoch milliseconds.
fn now_ms() -> u64 {
    system_time_ms(SystemTime::now())
}

/// Milliseconds since `started`, saturating.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Lock through a poison, rather than panicking a worker because another one already died.
fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
