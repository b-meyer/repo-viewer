//! One debounced watcher over every repository.
//!
//! `notify` spawns a thread per `Watcher` object, so a watcher per repository would mean 300
//! threads for 300 repositories. There is exactly one here, and [`RepoWatcher::sync`] calls
//! `watch()` once per path in each repository's [`watch_set`].
//!
//! # Debouncing is mandatory, not an optimisation
//!
//! Git does not write `.git/index` once. It writes `index.lock`, writes, then renames — so a
//! single `git add` produces a create/modify/remove burst, and an undebounced watcher would run
//! three refreshes for it. `notify-debouncer-full` collapses the burst; a per-repository cooldown
//! on the consumer side collapses whatever survives.
//!
//! # This reports repositories, not files
//!
//! A caller has no use for `…/.git/refs/remotes/origin/main`. The reverse index built during
//! [`RepoWatcher::sync`] maps every watched path back to the repositories that asked for it, and
//! that mapping is the whole reason this type owns the debouncer rather than handing one out: the
//! index and the registrations cannot drift if one operation maintains both.
//!
//! # Watching is never the source of truth
//!
//! `notify`'s own documentation warns that a backend "may fail to receive all events" at high file
//! counts and that backends are "not a 100% reliable source". A caller must still ship the poll and
//! the refresh-on-focus. When a backend says outright that it lost track — an event carrying
//! `Flag::Rescan` — every watched repository is reported as changed, because after a rescan that is
//! the only honest answer.

mod set;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::Duration,
};

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::{
    error::Result,
    model::{DiscoveredRepo, ScanError},
};

pub use set::{Watch, watch_set};

/// What a watcher has to say. Both variants carry rendered values, never `notify` types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// These repositories changed, by canonical repository path.
    ///
    /// Already deduplicated across the debounced batch, so a burst that touched `HEAD`, `index` and
    /// two refs in one repository arrives as one path.
    Changed(Vec<PathBuf>),

    /// Watching failed or degraded, with what to do about it where there is anything to say.
    ///
    /// Not fatal: the poll and the focus refresh are exactly the fallback this is a signal to lean
    /// on, so a caller surfaces it and carries on rather than treating it as the end of updates.
    Failed(String),
}

/// The one watcher, its registrations, and the index that maps them back to repositories.
///
/// Held for the life of the session by whoever built it. `Debouncer`'s `Drop` stops its thread, so
/// letting this fall out of scope stops watching — silently, since nothing fails.
pub struct RepoWatcher {
    /// The debouncer, which owns the single `notify` watcher underneath it.
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,

    /// Watched path → the repositories that asked for it.
    ///
    /// **Plural, and that is the point.** A linked worktree and the repository it was linked from
    /// share a common directory, so one update under `refs/remotes/` there moves both their
    /// ahead/behind counts and both need re-reading. It doubles as the reference count: a path is
    /// registered when its first repository arrives and unwatched when its last one goes.
    ///
    /// Shared with the debouncer's handler thread, which is why it is behind a lock rather than
    /// owned outright.
    index: Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>>,

    /// Repository path → the paths registered for it, shallowest first.
    registered: HashMap<PathBuf, Vec<PathBuf>>,
}

impl std::fmt::Debug for RepoWatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepoWatcher")
            .field("repos", &self.registered.len())
            .field("paths", &self.watched_paths())
            .finish_non_exhaustive()
    }
}

impl RepoWatcher {
    /// Build the watcher. Registers nothing — call [`RepoWatcher::sync`] with the repositories.
    ///
    /// `debounce` is the window after which a collapsed burst is delivered; ~300–500 ms is the
    /// range §7.3 asks for. The tick rate is left to the crate, which takes a quarter of the
    /// window.
    ///
    /// `on_event` runs on the debouncer's own thread and **must not block**. If it stalls, OS
    /// events pile up in the kernel buffer and are silently dropped on overflow — so the only
    /// correct body for it is one that hands the paths to somebody else.
    pub fn new<F>(debounce: Duration, on_event: F) -> Result<Self>
    where
        F: Fn(WatchEvent) + Send + 'static,
    {
        let index: Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>> = Arc::default();

        let handler_index = Arc::clone(&index);
        let debouncer = new_debouncer(debounce, None, move |result: DebounceEventResult| {
            match result {
                Ok(events) => {
                    let changed = resolve(&handler_index, &events);
                    if !changed.is_empty() {
                        on_event(WatchEvent::Changed(changed));
                    }
                }
                // Runtime failures from the backend, as opposed to the registration failures
                // `sync` returns. One event per error rather than a joined string, so a caller
                // showing only the most recent shows a whole cause rather than a fragment.
                Err(errors) => {
                    for error in &errors {
                        on_event(WatchEvent::Failed(render(error)));
                    }
                }
            }
        })?;

        Ok(Self {
            debouncer,
            index,
            registered: HashMap::new(),
        })
    }

    /// Make the registrations match `repos`, and report the ones that could not be made.
    ///
    /// A diff rather than a rebuild. Tearing the watcher down and building it back up would be
    /// fewer lines and would leave a window — however short — in which a change to a repository
    /// nobody was watching goes unnoticed and unrecoverable, since a missed event is not resent.
    ///
    /// Failures come back as [`ScanError`] values rather than ending the sync, on discovery's
    /// principle: one repository on a disconnected share must not cost the other 299 their watches.
    ///
    /// # Ordering
    ///
    /// Removals happen before additions, and removed paths are unwatched **deepest first**.
    /// `Debouncer::unwatch` drops its record of every root that `starts_with` the path it is
    /// given, so unwatching a Git directory before the `refs/` tree inside it would discard the
    /// bookkeeping for a watch that is still registered with the OS.
    pub fn sync(&mut self, repos: &[DiscoveredRepo]) -> Vec<ScanError> {
        let wanted: HashSet<&Path> = repos.iter().map(|repo| repo.path.as_path()).collect();

        let gone: Vec<PathBuf> = self
            .registered
            .keys()
            .filter(|path| !wanted.contains(path.as_path()))
            .cloned()
            .collect();
        for repo in &gone {
            self.forget(repo);
        }

        let mut errors = Vec::new();
        for repo in repos {
            if self.registered.contains_key(&repo.path) {
                continue;
            }
            self.register(repo, &mut errors);
        }
        errors
    }

    /// How many repositories are being watched.
    pub fn watched_repos(&self) -> usize {
        self.registered.len()
    }

    /// How many paths are registered with the OS.
    ///
    /// Lower than three times the repository count whenever worktrees share a common directory, and
    /// whenever a repository has no `logs/HEAD` yet. This is the number that matters against a
    /// platform's watch limit, so it is the one reported.
    pub fn watched_paths(&self) -> usize {
        self.read_index().len()
    }

    /// Register one repository's watch set, recording each path that took.
    fn register(&mut self, repo: &DiscoveredRepo, errors: &mut Vec<ScanError>) {
        let mut taken = Vec::with_capacity(5);

        for (path, mode) in watch_set(repo) {
            // Already registered by another repository sharing it — a linked worktree's common
            // directory. Watching it twice would ask the OS for a second handle on the same
            // directory to no purpose; the index entry below is what makes the sharing work.
            let fresh = !self.read_index().contains_key(&path);
            if fresh && let Err(error) = self.watch(&path, mode) {
                errors.push(ScanError {
                    path: repo.path.clone(),
                    message: render(&error),
                });
                continue;
            }

            self.write_index()
                .entry(path.clone())
                .or_default()
                .push(repo.path.clone());
            taken.push(path);
        }

        self.registered.insert(repo.path.clone(), taken);
    }

    /// Drop one repository's registrations, unwatching the paths nothing else wants.
    fn forget(&mut self, repo: &Path) {
        let Some(paths) = self.registered.remove(repo) else {
            return;
        };

        let mut orphaned: Vec<PathBuf> = Vec::new();
        {
            let mut index = self.write_index();
            for path in paths {
                let Some(holders) = index.get_mut(&path) else {
                    continue;
                };
                holders.retain(|holder| holder != repo);
                if holders.is_empty() {
                    index.remove(&path);
                    orphaned.push(path);
                }
            }
        }

        // Deepest first. See this method's note on `unwatch`'s cascading bookkeeping.
        orphaned.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for path in orphaned {
            if let Err(error) = self.debouncer.unwatch(&path) {
                // Nothing a caller can do about it and nothing a user needs to see: the watch is
                // gone from the index either way, so at worst an event arrives for a repository no
                // longer in it and resolves to nothing.
                tracing::debug!(path = %path.display(), %error, "could not unwatch");
            }
        }
    }

    /// Register one path.
    ///
    /// `Debouncer::watch`, **not** `.watcher().watch(…)`: 0.7 moved every `Watcher` method onto the
    /// debouncer itself and deprecated the accessor, and this workspace builds clippy at
    /// `-D warnings`, so the older idiom every example still shows does not compile here.
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        self.debouncer.watch(path, mode)
    }

    /// Read the index, recovering from a poisoned lock.
    fn read_index(&self) -> RwLockReadGuard<'_, HashMap<PathBuf, Vec<PathBuf>>> {
        self.index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Write the index, recovering from a poisoned lock.
    fn write_index(&self) -> RwLockWriteGuard<'_, HashMap<PathBuf, Vec<PathBuf>>> {
        self.index
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Turn a debounced batch into the repositories it belongs to.
///
/// Each event path is resolved by walking its ancestors against the index, which is a hash lookup
/// per directory level rather than a comparison against every watched repository. Deduplicated with
/// insertion order preserved, so a caller refreshing in order refreshes in the order things
/// happened.
///
/// An event flagged as a rescan means the backend lost track of what it was watching, so every
/// watched repository is returned: after a rescan, "nothing else changed" is not a claim anyone can
/// make.
fn resolve(
    index: &Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>>,
    events: &[notify_debouncer_full::DebouncedEvent],
) -> Vec<PathBuf> {
    let index = index
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if events.iter().any(|event| event.need_rescan()) {
        tracing::warn!(
            "the watcher lost track and asked for a rescan; refreshing every repository"
        );
        let mut all: Vec<PathBuf> = index.values().flatten().cloned().collect();
        all.sort_unstable();
        all.dedup();
        return all;
    }

    let mut seen: HashSet<&Path> = HashSet::new();
    let mut changed: Vec<PathBuf> = Vec::new();
    for path in events.iter().flat_map(|event| event.paths.iter()) {
        for ancestor in path.ancestors() {
            if let Some(holders) = index.get(ancestor) {
                for holder in holders {
                    if seen.insert(holder.as_path()) {
                        changed.push(holder.clone());
                    }
                }
                break;
            }
        }
    }
    changed
}

/// Render a `notify` failure, with the platform's fix appended where there is one.
///
/// The kind is matched rather than the message: `ErrorKind::MaxFilesWatch` is what `notify`'s
/// inotify backend maps `ENOSPC` onto, and its own rendering — "OS file watch limit reached." —
/// gives a user nothing to act on. On Linux that limit is the single most likely way watching
/// fails on a large tree, and it is raisable in one command.
fn render(error: &notify::Error) -> String {
    let mut message = error.to_string();
    if matches!(error.kind, notify::ErrorKind::MaxFilesWatch) {
        message.push_str(
            " Raise it with `sysctl fs.inotify.max_user_watches=524288` \
             (persist it in /etc/sysctl.d/). Until then the 60-second poll and \
             refresh-on-focus are what keep rows current.",
        );
    }
    message
}
