//! Repository discovery: the parallel walk that finds repositories under a set of roots.
//!
//! This stage touches no Git objects and opens no repository. It answers only "where are the
//! repositories", and it streams, because the whole design rests on a row appearing before
//! anything expensive happens to it.
//!
//! # Why `ignore` and not `walkdir`
//!
//! `walkdir` is a sequential iterator. `rayon` can parallelise work over the entries it yields,
//! but not the directory *descent*, which is the bottleneck.
//! `ignore::WalkBuilder::build_parallel` — the crate behind ripgrep — parallelises the descent
//! itself and offers a prune predicate.
//!
//! The walk runs with every standard filter off. It must be able to *see* hidden and ignored
//! directories in order to prune them: this is not a content search, and a `.gitignore` that
//! excludes a vendored directory must not be allowed to hide a repository inside it.
//!
//! # Failures are values
//!
//! A permission error, an unreadable root, or a broken `.git` lands in [`ScanSummary::errors`] and
//! the walk carries on. Discovery has no fallible signature at all, because there is no failure
//! that should cost the user the repositories that *were* found: a scan of five roots where one is
//! a stale drive letter must still return the other four.

mod classify;
mod prune;

use std::{
    collections::HashSet,
    num::NonZero,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Instant,
};

use ignore::{WalkBuilder, WalkState};

use crate::model::{DiscoveredRepo, RepoKind, ScanError, ScanOpts, ScanSummary};

/// Walk `roots` and collect every repository beneath them.
///
/// Results are sorted by path. A parallel walk yields in a nondeterministic order, and a caller
/// that wants a stable list should not have to sort it themselves. Use [`discover_roots_with`]
/// instead when rows should stream as they are found.
///
/// `should_interrupt` is taken here as well as on the streaming variant deliberately: an API where
/// only one of the two can be cancelled is a trap for the poll refresh that reaches for the
/// collecting one.
pub fn discover_roots(
    roots: &[PathBuf],
    opts: &ScanOpts,
    should_interrupt: &AtomicBool,
) -> (Vec<DiscoveredRepo>, ScanSummary) {
    let found = Mutex::new(Vec::new());
    let summary = discover_roots_with(roots, opts, should_interrupt, |repo| {
        lock(&found).push(repo);
    });

    let mut repos = found.into_inner().unwrap_or_else(PoisonError::into_inner);
    repos.sort_by(|left, right| left.path.cmp(&right.path));
    (repos, summary)
}

/// Walk `roots`, calling `on_repo` once per repository as it is found.
///
/// `on_repo` runs on a walker thread and is called concurrently, so it must be cheap: anything
/// slow done here stalls the descent. The IPC layer batches these into channel sends rather than
/// sending one per repository.
///
/// Each repository is reported exactly once. Deduplication is on the `dunce`-canonicalised path,
/// which is what collapses overlapping roots, case-differing roots, and — when `follow_links` is
/// on — two links to one directory into a single row.
///
/// `should_interrupt` is checked once per entry. A cancelled walk keeps the repositories it had
/// already reported and is not an error — the caller flipped the flag and already knows.
/// `dirs_visited` under-reports on a cancelled walk, which is correct: it counts the directories
/// actually examined.
pub fn discover_roots_with<F>(
    roots: &[PathBuf],
    opts: &ScanOpts,
    should_interrupt: &AtomicBool,
    on_repo: F,
) -> ScanSummary
where
    F: Fn(DiscoveredRepo) + Send + Sync,
{
    let started = Instant::now();
    let errors = Mutex::new(Vec::new());
    let roots = prepare_roots(roots, &errors);

    let mut summary = ScanSummary::default();
    if roots.is_empty() {
        summary.errors = errors.into_inner().unwrap_or_else(PoisonError::into_inner);
        summary.elapsed_ms = elapsed_ms(started);
        return summary;
    }

    let seen = Mutex::new(HashSet::new());
    let visited = AtomicU32::new(0);
    let pruned = Arc::new(AtomicU32::new(0));
    let descend_into_repos = opts.descend_into_repos;

    // One walk over every root rather than one walk each, so the thread pool is built once and the
    // deduplication set is shared across roots.
    let mut builder = WalkBuilder::new(&roots[0]);
    for root in &roots[1..] {
        builder.add(root);
    }
    builder
        // Turns off hidden, parents, ignore, git_ignore, git_global and git_exclude together. The
        // two the design actually depends on are restated so the intent survives a refactor.
        .standard_filters(false)
        .hidden(false)
        .git_ignore(false)
        .follow_links(opts.follow_links)
        .same_file_system(opts.same_file_system)
        .max_depth(opts.max_depth.map(|depth| depth as usize))
        .threads(walker_threads(opts.threads))
        .filter_entry(prune::build(&opts.prune_names, Arc::clone(&pruned)));

    builder.build_parallel().run(|| {
        let (seen, errors, visited, on_repo) = (&seen, &errors, &visited, &on_repo);

        Box::new(move |result| {
            // First statement in the visitor, deliberately. `WalkState::Quit` is documented as
            // asynchronous — more entries arrive after it — so checking here drops them instead of
            // reporting them. A repository delivered after its scan has already emitted `Cancelled`
            // would have nowhere to go.
            if should_interrupt.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }

            let entry = match result {
                Ok(entry) => entry,
                Err(err) => {
                    push_error(
                        errors,
                        error_path(&err).unwrap_or_default(),
                        err.to_string(),
                    );
                    return WalkState::Continue;
                }
            };

            // Only a directory can be a repository.
            if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                return WalkState::Continue;
            }
            visited.fetch_add(1, Ordering::Relaxed);

            let found = match classify::classify(entry.path()) {
                Ok(Some(found)) => found,
                Ok(None) => return WalkState::Continue,
                Err(message) => {
                    push_error(errors, entry.path().to_path_buf(), message);
                    return WalkState::Continue;
                }
            };

            let path = canonical(entry.path());
            if !lock(seen).insert(path.clone()) {
                // Reached a second time, through an overlapping root or a link. Still stop the
                // descent: the first visit already decided what happens below here.
                return WalkState::Skip;
            }

            on_repo(DiscoveredRepo {
                name: display_name(&path),
                parent: path
                    .parent()
                    .map_or_else(|| path.clone(), Path::to_path_buf),
                path,
                kind: found.kind,
                git_dir: canonical(&found.git_dir),
                common_dir: canonical(&found.common_dir),
            });

            // A bare repository is itself a Git directory, so everything under it is object
            // storage. It is never descended into, regardless of the flag.
            if found.kind == RepoKind::Bare || !descend_into_repos {
                WalkState::Skip
            } else {
                WalkState::Continue
            }
        })
    });

    summary.repos_found = lock(&seen).len() as u32;
    summary.dirs_visited = visited.load(Ordering::Relaxed);
    summary.dirs_pruned = pruned.load(Ordering::Relaxed);
    summary.errors = errors.into_inner().unwrap_or_else(PoisonError::into_inner);
    summary.elapsed_ms = elapsed_ms(started);
    summary
}

/// Canonicalise, validate, and deduplicate the configured roots.
///
/// A root that cannot be read is recorded and skipped rather than ending the scan.
fn prepare_roots(roots: &[PathBuf], errors: &Mutex<Vec<ScanError>>) -> Vec<PathBuf> {
    let mut usable: Vec<PathBuf> = Vec::new();
    for root in roots {
        match dunce::canonicalize(root) {
            Ok(path) if path.is_dir() => {
                if !usable.contains(&path) {
                    usable.push(path);
                }
            }
            Ok(path) => push_error(errors, path, "not a directory".to_string()),
            Err(err) => push_error(errors, root.clone(), err.to_string()),
        }
    }
    usable
}

/// How many threads the walker gets.
///
/// Halved deliberately when unspecified. `ignore` and `rayon` both default to the full core count,
/// and from Tier 0 onwards the two run at once — the walk is still descending while found
/// repositories are being read — so taking each default would put twice as many threads on the
/// machine as it has cores.
fn walker_threads(requested: Option<u32>) -> usize {
    if let Some(threads) = requested {
        return (threads as usize).max(1);
    }
    let cores = std::thread::available_parallelism().map_or(1, NonZero::get);
    (cores / 2).max(1)
}

/// Canonicalise through `dunce`, falling back to the path as given.
///
/// `std::fs::canonicalize` *returns* verbatim paths on Windows. They render badly, confuse `git`
/// CLI arguments, and compare unequal to the same path typed normally — which matters here because
/// this value is the map key. `dunce` drops the prefix whenever the path is representable without
/// it.
///
/// Public because a configured root has to be normalised the *same* way a discovered repository
/// is. They are compared against each other — a root is a path prefix of the rows beneath it — so
/// two spellings of one folder would silently stop matching. Callers outside the walk get this
/// function rather than their own `dunce` call for exactly that reason.
pub fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The final component, for display. Falls back to the whole path for a filesystem root.
fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// Dig the path out of an `ignore::Error`, which nests it up to three layers deep and exposes no
/// accessor for it.
fn error_path(err: &ignore::Error) -> Option<PathBuf> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path.clone()),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        ignore::Error::Loop { child, .. } => Some(child.clone()),
        _ => None,
    }
}

/// Record a non-fatal failure.
fn push_error(errors: &Mutex<Vec<ScanError>>, path: PathBuf, message: String) {
    lock(errors).push(ScanError { path, message });
}

/// Lock through a poison, rather than panicking a walker thread because another one already died.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Milliseconds since `started`, saturating.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
