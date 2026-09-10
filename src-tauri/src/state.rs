//! The canonical row state, the configured roots, and the live scans.
//!
//! Rust owns the one `HashMap<PathBuf, RepoStatus>`. Each tier result is merged into it here and
//! the **full merged row** is what goes over the channel; the Pinia store is a mirror that never
//! merges and never holds a value this map does not. That is the whole reason the merge lives on
//! this side: a tiered stream delivers results out of order, and a frontend that tried to
//! reconcile them would eventually show a stale value as a fresh one.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Sender,
    },
};

use repo_scan::{
    DiscoveredRepo, RepoEvent, RepoStatus, RepoWatcher, ScanError, ScanId, Tier1, Tier2,
};
use tauri::ipc::Channel;

use crate::live::Tick;

/// Everything the app owns between commands.
///
/// Managed as `Arc<AppState>` rather than as `AppState`: `State<'r, T>` borrows the manager, and
/// the scan pipeline runs on `spawn_blocking` and needs an owned `'static` handle. The `Arc` is
/// also what lets `pipeline::run_scan` take a plain `Arc<AppState>` and be exercised with no Tauri
/// application at all.
#[derive(Default)]
pub struct AppState {
    /// The canonical rows, keyed by the same absolute path the frontend uses.
    ///
    /// `RwLock` and not `Mutex` because path validation and the cache snapshot are both reads.
    /// Not `dashmap`: the write side takes the lock once per batch rather than once per row, so
    /// per-key sharding buys nothing here — and a sharded map cannot cheaply produce the coherent
    /// snapshot the cache needs.
    repos: RwLock<HashMap<PathBuf, RepoStatus>>,

    /// What discovery found, keyed the same way as [`AppState::repos`].
    ///
    /// A second map rather than a field on the row, because the row does not have one to spare:
    /// `RepoStatus` carries no `git_dir`, and every engine entry point needs the *resolved* one —
    /// `<path>/.git` for a normal repository, the path itself when bare, and the private directory
    /// a `.git` file names for a worktree or submodule. Discovery resolved it once and re-resolving
    /// it per command is explicitly not done, so it is kept here instead.
    ///
    /// This is also a **superset** of `repos`: a repository whose HEAD could not be read has an
    /// entry here and no row, which is what gives `refresh_repo` something to retry.
    found: RwLock<HashMap<PathBuf, DiscoveredRepo>>,

    /// The configured roots, canonicalised.
    ///
    /// Persisted by [`crate::persist`] from the two commands that change them, and read back into
    /// here by `setup` before any command can run.
    roots: RwLock<Vec<PathBuf>>,

    /// Live scans and their cancellation flags. An entry exists exactly while its pipeline runs.
    scans: Mutex<HashMap<ScanId, Arc<AtomicBool>>>,

    /// Hands out [`ScanId`]s. Starts at 1, so `0` is never a live scan.
    next_scan: AtomicU64,

    /// The session channel opened by `subscribe`.
    ///
    /// Held by value, which is load-bearing. A `Channel` taken as a command argument installs an
    /// `on_drop` hook that evals `{ end: true }` into the webview, and `ChannelInner`'s `Drop`
    /// fires when the last clone goes — so using the argument and letting it fall out of scope at
    /// the end of `subscribe` would close the channel the instant it was opened.
    session: Mutex<Option<Channel<RepoEvent>>>,

    /// The one filesystem watcher, once [`crate::live::start`] has built it.
    ///
    /// Held here for the same reason the session channel is: `Debouncer`'s `Drop` stops its thread,
    /// so a watcher owned by the function that created it would stop watching the moment that
    /// function returned.
    ///
    /// `Option` because construction is fallible and because `None` is a state the app genuinely
    /// runs in — watching turned off in the settings file, or a backend that refused to start. The
    /// poll and the refresh-on-focus are what make that survivable, which is why nothing here
    /// treats a missing watcher as an error.
    watcher: Mutex<Option<RepoWatcher>>,

    /// Wakes the poll thread, once [`crate::live::start`] has spawned it.
    ///
    /// The focus refresh and the shutdown signal both go through here, which is what makes them the
    /// poll's own code path rather than two more of their own.
    poll: Mutex<Option<Sender<Tick>>>,
}

/// Hand-written because `tauri::ipc::Channel` implements no `Debug`, and because dumping every row
/// would make the derived output useless anyway. Reports sizes, which is what a log line wants.
impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppState")
            .field("repos", &self.read_repos().len())
            .field("found", &self.read_found().len())
            .field("roots", &self.read_roots().len())
            .field("scans", &self.lock_scans().len())
            .field("session", &self.lock_session().is_some())
            .field(
                "watching",
                &self
                    .lock_watcher()
                    .as_ref()
                    .map_or(0, RepoWatcher::watched_paths),
            )
            .finish()
    }
}

impl AppState {
    /// Merge a batch of Tier 0 rows into the canonical map and return the full merged rows.
    ///
    /// Takes the write lock once for the whole batch rather than once per row, which is the point
    /// of batching on the write side as well as on the wire.
    pub fn merge_tier0_batch(&self, rows: Vec<RepoStatus>) -> Vec<RepoStatus> {
        let mut repos = self.write_repos();
        rows.into_iter()
            .map(|row| {
                let merged = merge_tier0(repos.get(&row.path), row);
                repos.insert(merged.path.clone(), merged.clone());
                merged
            })
            .collect()
    }

    /// Merge a batch of Tier 1 results into the canonical map and return the full merged rows.
    ///
    /// A result for a path the map does not hold is dropped rather than inserted: Tier 1 cannot
    /// produce a row on its own — it has no `head` — so there is nothing honest to insert. That
    /// only happens if Tier 0 failed on the repository, in which case the absence of a row is
    /// itself the signal.
    pub fn merge_tier1_batch(&self, rows: Vec<Tier1>) -> Vec<RepoStatus> {
        let mut repos = self.write_repos();
        rows.into_iter()
            .filter_map(|row| {
                let existing = repos.get(&row.path)?;
                let merged = merge_tier1(existing, row);
                repos.insert(merged.path.clone(), merged.clone());
                Some(merged)
            })
            .collect()
    }

    /// Merge one Tier 2 read into the canonical map and return the full merged row.
    ///
    /// Single rather than batched, unlike the other two: Tier 2 runs for one expanded row at a
    /// time, so there is no batch to take the lock once for.
    ///
    /// `None` for a path the map does not hold, for the same reason [`AppState::merge_tier1_batch`]
    /// drops one: Tier 2 has no `head` and so cannot produce a row on its own.
    pub fn merge_tier2(&self, row: Tier2) -> Option<RepoStatus> {
        let mut repos = self.write_repos();
        let existing = repos.get(&row.path)?;
        let merged = merge_tier2(existing, row);
        repos.insert(merged.path.clone(), merged.clone());
        Some(merged)
    }

    /// Drop Tier 2's fields from these rows, because something changed underneath them.
    ///
    /// **The one deliberate exception to tier ownership**, and it is not a softening of the rule
    /// but the honesty rule the merge exists to serve, applied where the merge cannot reach. A
    /// watcher or a poll re-reads Tiers 0 and 1; tier ownership therefore leaves `counts` and
    /// `submodules` exactly as they were, which for a repository that just changed means a
    /// pre-change count sitting beside a post-change branch. The drawer shows those counts with no
    /// age beside them, so the pair reads as one freshly measured moment. `persist.rs` drops the
    /// same two fields on the load path for the same reason.
    ///
    /// Deliberately **not** folded into [`AppState::merge_tier0_batch`]: an explicit
    /// `refresh_repo(path, Tier::Zero)` asked for refs and nothing more, and must leave a Tier 2
    /// read alone. The difference is the trigger, not the tier — so it is a separate call the two
    /// unsolicited triggers make and the command does not.
    ///
    /// Sends nothing. The caller re-reads immediately afterwards and pushes one merged row, so a
    /// push here would put a row on the wire that is emptier than anything a user should see.
    ///
    /// A path with no row is skipped: there is nothing to invalidate, and no row to invent.
    pub fn invalidate_tier2(&self, paths: &[PathBuf]) {
        let mut repos = self.write_repos();
        for path in paths {
            if let Some(existing) = repos.get(path) {
                let cleared = RepoStatus {
                    counts: None,
                    submodules: None,
                    ..existing.clone()
                };
                repos.insert(path.clone(), cleared);
            }
        }
    }

    /// Record what discovery found, so a later per-row read can reach its resolved Git directory.
    pub fn record_found(&self, repos: &[DiscoveredRepo]) {
        let mut found = self.write_found();
        for repo in repos {
            found.insert(repo.path.clone(), repo.clone());
        }
    }

    /// What discovery found for `path`, if anything.
    ///
    /// The validator for `refresh_repo`, and the only source of a `git_dir` outside a scan.
    pub fn discovered(&self, path: &Path) -> Option<DiscoveredRepo> {
        self.read_found().get(path).cloned()
    }

    /// Every entry discovery produced, sorted by path.
    ///
    /// The other half of the cache snapshot. Sorted for the same reason [`AppState::snapshot`] is:
    /// the file is rewritten whole on every scan, and an unstable order would make its diff mean
    /// nothing.
    pub fn discovered_all(&self) -> Vec<DiscoveredRepo> {
        let mut found: Vec<DiscoveredRepo> = self.read_found().values().cloned().collect();
        found.sort_by(|left, right| left.path.cmp(&right.path));
        found
    }

    /// Whether `path` is a key of the canonical map.
    ///
    /// The validator for commands that need an existing row to work on. A repository discovery
    /// found but Tier 0 could not read is **not** here — see [`AppState::discovered`].
    pub fn has_repo(&self, path: &Path) -> bool {
        self.read_repos().contains_key(path)
    }

    /// The canonical row for `path`, if there is one.
    ///
    /// For a command that has to answer with a row it did not change — a Tier 2 read of a bare
    /// repository, where there is nothing to compute and nothing to merge.
    pub fn row(&self, path: &Path) -> Option<RepoStatus> {
        self.read_repos().get(path).cloned()
    }

    /// Record a per-repository failure against rows that already exist, and return them merged.
    ///
    /// For a tier that failed outright on a repository Tier 0 *had* read: the row is real and
    /// keeps every value it has, so the cause belongs on it rather than in a summary listing
    /// repositories that produced no row at all. A path with no row is skipped — that case is
    /// already described by the absence.
    pub fn record_errors(&self, failures: &[ScanError]) -> Vec<RepoStatus> {
        let mut repos = self.write_repos();
        failures
            .iter()
            .filter_map(|failure| {
                let existing = repos.get(&failure.path)?;
                let merged = RepoStatus {
                    error: join_errors(existing.error.as_deref(), Some(&failure.message)),
                    ..existing.clone()
                };
                repos.insert(merged.path.clone(), merged.clone());
                Some(merged)
            })
            .collect()
    }

    /// Every row, sorted by path.
    ///
    /// `subscribe` returns this so a reloaded webview — which happens constantly under HMR —
    /// repaints from the canonical map instead of forcing a rescan.
    pub fn snapshot(&self) -> Vec<RepoStatus> {
        let mut rows: Vec<RepoStatus> = self.read_repos().values().cloned().collect();
        rows.sort_by(|left, right| left.path.cmp(&right.path));
        rows
    }

    /// Seed both maps from the persisted cache.
    ///
    /// Insertion rather than a merge, which is correct here and nowhere else: this runs once from
    /// `setup`, before any command can be invoked and so before any tier can have written a row.
    ///
    /// `scanned_at_ms` is left exactly as it was written. The age of a cached row is what the table
    /// renders beside it, so stamping it fresh here would turn last week's answer into this
    /// morning's — the same lie as rendering an uncomputed count as `0`.
    pub fn restore(&self, rows: Vec<RepoStatus>, found: Vec<DiscoveredRepo>) {
        {
            let mut repos = self.write_repos();
            for row in rows {
                repos.insert(row.path.clone(), row);
            }
        }

        let mut discovered = self.write_found();
        for repo in found {
            discovered.insert(repo.path.clone(), repo);
        }
    }

    /// Add an already-canonicalised root. Returns the new list. Idempotent.
    pub fn add_root(&self, path: PathBuf) -> Vec<PathBuf> {
        let mut roots = self.write_roots();
        if !roots.contains(&path) {
            roots.push(path);
            roots.sort();
        }
        roots.clone()
    }

    /// Drop a root and evict every row beneath it.
    ///
    /// Returns the new root list and the evicted paths together, so the caller can answer its own
    /// invocation and push [`RepoEvent::Removed`] from one consistent view rather than taking the
    /// locks twice and racing itself.
    pub fn remove_root(&self, path: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let roots = {
            let mut roots = self.write_roots();
            roots.retain(|root| root != path);
            roots.clone()
        };

        // Both maps are keyed the same way and a root is a prefix of every key beneath it, which is
        // what `add_root`'s canonicalisation buys. `found` is evicted too, or a removed root would
        // leave behind exactly the entries that let a command reach into it.
        let mut repos = self.write_repos();
        let evicted: Vec<PathBuf> = repos
            .keys()
            .filter(|key| key.starts_with(path))
            .cloned()
            .collect();
        for key in &evicted {
            repos.remove(key);
        }

        self.write_found().retain(|key, _| !key.starts_with(path));

        (roots, evicted)
    }

    /// Drop rows a **completed** scan of `roots` did not see, and return their paths.
    ///
    /// Every other row change is an upsert, which is all a scan of a live tree needs. A scan that
    /// starts from cache-restored rows is the case that needs more: a repository deleted or moved
    /// between sessions has a row and no longer has a folder, and nothing short of removing its
    /// root would ever take that row away.
    ///
    /// Scoped to the roots that were walked, so scanning one root cannot evict another's rows — and
    /// only ever called for a scan that finished, because a cancelled walk has not seen the whole
    /// tree and what it missed is not the same as what is gone.
    pub fn retain_scanned(&self, roots: &[PathBuf], seen: &HashSet<PathBuf>) -> Vec<PathBuf> {
        let walked = |key: &Path| roots.iter().any(|root| key.starts_with(root));

        let evicted: Vec<PathBuf> = {
            let mut repos = self.write_repos();
            let gone: Vec<PathBuf> = repos
                .keys()
                .filter(|key| walked(key) && !seen.contains(*key))
                .cloned()
                .collect();
            for key in &gone {
                repos.remove(key);
            }
            gone
        };

        // `found` goes on the same terms, or a removed repository would keep exactly the entry
        // that lets a command reach into it.
        self.write_found()
            .retain(|key, _| !walked(key) || seen.contains(key));

        evicted
    }

    /// The configured roots.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.read_roots().clone()
    }

    /// Whether `path` is a configured root. `scan_roots` accepts nothing else.
    pub fn has_root(&self, path: &Path) -> bool {
        self.read_roots().iter().any(|root| root == path)
    }

    /// Allocate an id and register a fresh cancellation flag for it.
    pub fn begin_scan(&self) -> (ScanId, Arc<AtomicBool>) {
        let id = ScanId(self.next_scan.fetch_add(1, Ordering::Relaxed) + 1);
        let flag = Arc::new(AtomicBool::new(false));
        self.lock_scans().insert(id, Arc::clone(&flag));
        (id, flag)
    }

    /// Deregister a scan. Idempotent — the pipeline's guard calls it on the unwind path too.
    pub fn finish_scan(&self, id: ScanId) {
        self.lock_scans().remove(&id);
    }

    /// Flip one scan's flag.
    ///
    /// A no-op for an id that has already finished. That race is not avoidable from the frontend's
    /// side — it cannot know the scan ended between rendering the button and the click — so it is
    /// not an error either.
    pub fn cancel_scan(&self, id: ScanId) {
        if let Some(flag) = self.lock_scans().get(&id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Flip every live scan's flag. For a root change, a new scan, and window close.
    pub fn cancel_all(&self) {
        for flag in self.lock_scans().values() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Whether any scan is running.
    ///
    /// What the watcher and the poll check before doing anything: a scan is about to re-read every
    /// row in the tree, so refreshing one underneath it is duplicated I/O whose only effect is to
    /// put a second copy of the same answer on a second channel.
    pub fn scan_in_flight(&self) -> bool {
        !self.lock_scans().is_empty()
    }

    /// Install the watcher, replacing any previous one.
    ///
    /// The replaced watcher is dropped here, which stops its thread — so this is a handover rather
    /// than a leak, and calling it twice does not accumulate watchers.
    pub fn set_watcher(&self, watcher: RepoWatcher) {
        *self.lock_watcher() = Some(watcher);
    }

    /// Make the watch set match what discovery has found, and report what could not be watched.
    ///
    /// Empty when there is no watcher — which is not the same as "everything is watched", and is
    /// why the registration counts are logged here rather than inferred from an empty list by the
    /// caller. That log line is also the only place the path count is visible, and it is the number
    /// that matters against a platform's watch limit.
    pub fn sync_watches(&self) -> Vec<ScanError> {
        let found = self.discovered_all();
        let mut watcher = self.lock_watcher();
        let Some(watcher) = watcher.as_mut() else {
            return Vec::new();
        };

        let failures = watcher.sync(&found);
        tracing::debug!(
            repos = watcher.watched_repos(),
            paths = watcher.watched_paths(),
            failed = failures.len(),
            "watch set synced"
        );
        failures
    }

    /// Install the poll thread's sender.
    pub fn set_poll(&self, sender: Sender<Tick>) {
        *self.lock_poll() = Some(sender);
    }

    /// Wake the poll thread, if there is one.
    ///
    /// A send failure means the thread has already ended, which on the shutdown path is the
    /// expected outcome rather than a problem — so it is dropped rather than logged.
    pub fn tick(&self, tick: Tick) {
        if let Some(sender) = self.lock_poll().as_ref() {
            let _ = sender.send(tick);
        }
    }

    /// Stop watching and polling.
    ///
    /// Called on window close beside [`AppState::cancel_all`], and for the same reason: these
    /// threads should unwind while the runtime is still up rather than at process exit. Dropping
    /// the watcher stops the debouncer, which drops the sender its callback holds, which is what
    /// ends the refresh thread — so the one drop shuts down two of the three.
    pub fn stop_live(&self) {
        self.tick(Tick::Stop);
        if self.lock_watcher().take().is_some() {
            tracing::debug!("watcher stopped");
        }
    }

    /// Install the session channel, replacing any previous one.
    ///
    /// A webview reload calls `subscribe` again. Dropping the old channel ends it on the JS side,
    /// which is correct: the listener it belonged to is gone.
    pub fn set_session(&self, channel: Channel<RepoEvent>) {
        *self.lock_session() = Some(channel);
    }

    /// Push on the session channel, if one is open.
    ///
    /// `send`'s `Err` is not a liveness check — it returns `Ok(())` for a closed webview and only
    /// fails once the whole app is shutting down — so a failure here is logged and never surfaced.
    pub fn push(&self, event: RepoEvent) {
        if let Some(channel) = self.lock_session().as_ref()
            && let Err(error) = channel.send(event)
        {
            tracing::warn!(%error, "could not push on the session channel");
        }
    }

    /// Read the rows, recovering from a poisoned lock.
    ///
    /// One panicking thread must not take every command with it — the same reason the engine's
    /// walk recovers rather than cascading.
    fn read_repos(&self) -> RwLockReadGuard<'_, HashMap<PathBuf, RepoStatus>> {
        self.repos.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write the rows, recovering from a poisoned lock.
    fn write_repos(&self) -> RwLockWriteGuard<'_, HashMap<PathBuf, RepoStatus>> {
        self.repos.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read the discovered repositories, recovering from a poisoned lock.
    fn read_found(&self) -> RwLockReadGuard<'_, HashMap<PathBuf, DiscoveredRepo>> {
        self.found.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write the discovered repositories, recovering from a poisoned lock.
    fn write_found(&self) -> RwLockWriteGuard<'_, HashMap<PathBuf, DiscoveredRepo>> {
        self.found.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read the roots, recovering from a poisoned lock.
    fn read_roots(&self) -> RwLockReadGuard<'_, Vec<PathBuf>> {
        self.roots.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write the roots, recovering from a poisoned lock.
    fn write_roots(&self) -> RwLockWriteGuard<'_, Vec<PathBuf>> {
        self.roots.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lock the scan registry, recovering from a poisoned lock.
    fn lock_scans(&self) -> MutexGuard<'_, HashMap<ScanId, Arc<AtomicBool>>> {
        self.scans.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lock the session channel, recovering from a poisoned lock.
    fn lock_session(&self) -> MutexGuard<'_, Option<Channel<RepoEvent>>> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lock the watcher, recovering from a poisoned lock.
    fn lock_watcher(&self) -> MutexGuard<'_, Option<RepoWatcher>> {
        self.watcher.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lock the poll sender, recovering from a poisoned lock.
    fn lock_poll(&self) -> MutexGuard<'_, Option<Sender<Tick>>> {
        self.poll.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Merge one Tier 0 read into the row already held for that path.
///
/// **Tier ownership, not field-wise option preference.** Tier 0 owns every field it reads and
/// replaces all of them, `None` included. `upstream`, `ahead`, `behind`, `last_commit` and
/// `last_fetched_ms` are `Option` because the *answer* can be none — no upstream configured, never
/// fetched — not because the value might be uncomputed. Carrying a stale `Some` forward because a
/// fresh read said `None` would report a deleted upstream as live, which is the same lie as
/// rendering an uncomputed count as `0`, pointing the other way.
///
/// Fields owned by other tiers are kept from `existing`. That is the case this function exists
/// for: a Tier 0 result arriving after a Tier 1 result carries `dirty: None`, and a wholesale
/// replace would erase a value the UI is already showing.
///
/// Every field is listed and there is no `..incoming`. A field added to `RepoStatus` must be a
/// compile error here rather than a silent decision that Tier 0 owns it — the same discipline as
/// mapping `gix::state::InProgress` with no wildcard arm.
pub fn merge_tier0(existing: Option<&RepoStatus>, incoming: RepoStatus) -> RepoStatus {
    let Some(existing) = existing else {
        return incoming;
    };

    RepoStatus {
        // Identity. Discovery re-derives all of it on every scan, so the fresh copy wins.
        path: incoming.path,
        name: incoming.name,
        parent: incoming.parent,
        kind: incoming.kind,

        // Tier 0's own fields, `None` included.
        head: incoming.head,
        upstream: incoming.upstream,
        ahead: incoming.ahead,
        behind: incoming.behind,
        last_commit: incoming.last_commit,
        stash_count: incoming.stash_count,
        state: incoming.state,
        last_fetched_ms: incoming.last_fetched_ms,

        // Tier 1's.
        dirty: existing.dirty,
        conflicted: existing.conflicted,

        // Tier 2's.
        counts: existing.counts,
        submodules: existing.submodules.clone(),

        // The age the UI renders is the most recent read of *any* part of the row, so a Tier 0
        // result that lost a race with a later tier must not make the row look older than it is.
        scanned_at_ms: incoming.scanned_at_ms.max(existing.scanned_at_ms),

        // Tier 0 owns the slot outright and replaces it, which is what stops Tier 1's appending
        // from accumulating across rescans: every scan restarts the chain here. Tier 2 does not
        // write it at all (PLAN.md §12, items 6 and 7).
        error: incoming.error,
    }
}

/// Merge one Tier 1 read into the row Tier 0 produced.
///
/// Tier 1 owns `dirty` and `conflicted` and nothing else. Every Tier 0 field is taken from
/// `existing` untouched — the mirror image of [`merge_tier0`], and the pair is what makes the
/// arrival order of the two tiers stop mattering.
///
/// `scanned_at_ms` moves forward, because this read is newer than the row it lands on.
///
/// **`error` is appended to rather than replaced.** The two tiers describe different halves of the
/// row and can each fail independently, so a Tier 1 failure must not erase the reason Tier 0's
/// upstream is missing. Appending cannot accumulate across rescans: Tier 0 owns the slot outright
/// and replaces it, so every scan starts the chain again.
pub fn merge_tier1(existing: &RepoStatus, incoming: Tier1) -> RepoStatus {
    RepoStatus {
        dirty: Some(incoming.dirty),
        conflicted: Some(incoming.conflicted),
        scanned_at_ms: now_ms().max(existing.scanned_at_ms),
        error: join_errors(existing.error.as_deref(), incoming.error.as_deref()),
        ..existing.clone()
    }
}

/// Merge one Tier 2 read into the row the earlier tiers produced.
///
/// Tier 2 owns `counts` and `submodules` and nothing else. In particular it does **not** write
/// `conflicted`, even though it counted one: Tier 1 owns that field, reaching the same number
/// through index stage entries, and two writers for one field is how the two answers get to
/// disagree. `FileCounts::conflicted` stays as the cross-check the engine's tests assert on.
///
/// **`error` is left alone**, which is the one place this differs from [`merge_tier1`]. Tier 2 runs
/// on demand and repeatedly — once per expand — so appending would stack a message per expand with
/// nothing but a rescan to clear it, and replacing would erase the reason an earlier tier's field
/// is missing. A Tier 2 failure has a caller waiting on a return value, so it is reported there
/// instead and never lands on the row (PLAN.md §12, item 7).
///
/// `..existing.clone()` rather than [`merge_tier0`]'s exhaustive field list, and the asymmetry is
/// deliberate: the fall-through here means "not Tier 2's", so a field added to `RepoStatus` is kept
/// from the row by default — which is the safe answer for the narrowest tier. Tier 0 lists every
/// field precisely because its fall-through would be the *unsafe* one.
pub fn merge_tier2(existing: &RepoStatus, incoming: Tier2) -> RepoStatus {
    RepoStatus {
        counts: incoming.counts,
        submodules: incoming.submodules,
        scanned_at_ms: now_ms().max(existing.scanned_at_ms),
        ..existing.clone()
    }
}

/// Combine two tiers' causes into the row's one `error` slot.
fn join_errors(existing: Option<&str>, incoming: Option<&str>) -> Option<String> {
    match (existing, incoming) {
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_string()),
        (None, None) => None,
    }
}

/// Now, as epoch milliseconds.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use repo_scan::{FileCounts, Head, RepoKind, RepoState};

    use super::*;

    /// A minimal row. Every field is public, so a test overrides only what it is about.
    ///
    /// `pub(super)` so the Tier 1 tests below can build the same shape.
    pub(super) fn row(path: &str) -> RepoStatus {
        RepoStatus {
            path: PathBuf::from(path),
            name: "repo".into(),
            parent: PathBuf::from("C:/work"),
            kind: RepoKind::Normal,
            head: Head::Branch {
                name: "main".into(),
            },
            upstream: None,
            ahead: None,
            behind: None,
            last_commit: None,
            stash_count: 0,
            state: RepoState::Clean,
            last_fetched_ms: None,
            dirty: None,
            conflicted: None,
            counts: None,
            submodules: None,
            scanned_at_ms: 1_000,
            error: None,
        }
    }

    /// §6.3's stated bug, and the reason the merge exists. **A wholesale replace fails this.**
    #[test]
    fn tier0_merge_keeps_a_tier1_value_the_new_row_does_not_carry() {
        let mut existing = row("C:/work/a");
        existing.dirty = Some(true);
        existing.conflicted = Some(2);
        existing.counts = Some(FileCounts::default());
        existing.submodules = Some(Vec::new());

        let merged = merge_tier0(Some(&existing), row("C:/work/a"));

        assert_eq!(merged.dirty, Some(true));
        assert_eq!(merged.conflicted, Some(2));
        assert_eq!(merged.counts, Some(FileCounts::default()));
        assert_eq!(merged.submodules, Some(Vec::new()));
    }

    /// The other half, and the one a careless reading of "merged field-wise" gets wrong.
    ///
    /// **A merge that prefers `Some` over `None` on every `Option` fails this**, and would report a
    /// deleted upstream as live for the rest of the session.
    #[test]
    fn tier0_merge_clears_an_upstream_that_is_gone() {
        let mut existing = row("C:/work/a");
        existing.upstream = Some("origin/main".into());
        existing.ahead = Some(3);
        existing.behind = Some(0);
        existing.last_fetched_ms = Some(500);

        let merged = merge_tier0(Some(&existing), row("C:/work/a"));

        assert_eq!(merged.upstream, None);
        assert_eq!(merged.ahead, None);
        assert_eq!(merged.behind, None);
        assert_eq!(merged.last_fetched_ms, None);
    }

    #[test]
    fn tier0_merge_of_an_unknown_path_inserts_the_row_unchanged() {
        let incoming = row("C:/work/new");
        assert_eq!(merge_tier0(None, incoming.clone()), incoming);
    }

    /// A Tier 0 read that lost a race with a later tier must not make the row look older.
    #[test]
    fn tier0_merge_takes_the_later_scanned_at() {
        let mut existing = row("C:/work/a");
        existing.scanned_at_ms = 9_000;

        let merged = merge_tier0(Some(&existing), row("C:/work/a"));

        assert_eq!(merged.scanned_at_ms, 9_000);
    }

    /// The error belongs to the read that produced it, so a clean Tier 0 read clears a stale one.
    #[test]
    fn tier0_merge_replaces_the_error_from_the_tier_that_produced_it() {
        let mut existing = row("C:/work/a");
        existing.error = Some("stash read failed".into());

        let merged = merge_tier0(Some(&existing), row("C:/work/a"));

        assert_eq!(merged.error, None);
    }

    /// The "send the **full merged row**" invariant: what the caller gets back is exactly what the
    /// map now holds, so the mirror cannot diverge from the canonical copy.
    #[test]
    fn merge_tier0_batch_returns_exactly_what_the_map_now_holds() {
        let state = AppState::default();
        let mut first = row("C:/work/a");
        first.dirty = Some(true);
        state.merge_tier0_batch(vec![first, row("C:/work/b")]);

        let returned = state.merge_tier0_batch(vec![row("C:/work/a"), row("C:/work/c")]);

        assert_eq!(returned.len(), 2);
        assert_eq!(returned[0].dirty, Some(true), "the Tier 1 value survived");
        let held = state.snapshot();
        for row in &returned {
            assert!(held.contains(row), "returned row is not what the map holds");
        }
        assert_eq!(held.len(), 3);
    }

    #[test]
    fn remove_root_evicts_only_rows_beneath_it() {
        let state = AppState::default();
        state.add_root(PathBuf::from("C:/work"));
        state.add_root(PathBuf::from("C:/other"));
        state.merge_tier0_batch(vec![row("C:/work/a"), row("C:/work/b"), row("C:/other/c")]);

        let (roots, evicted) = state.remove_root(Path::new("C:/work"));

        assert_eq!(roots, vec![PathBuf::from("C:/other")]);
        assert_eq!(evicted.len(), 2);
        assert!(evicted.iter().all(|path| path.starts_with("C:/work")));
        let held = state.snapshot();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].path, PathBuf::from("C:/other/c"));
    }

    #[test]
    fn scan_ids_are_unique_and_start_at_one() {
        let state = AppState::default();
        let (first, _) = state.begin_scan();
        let (second, _) = state.begin_scan();

        assert_eq!(first, ScanId(1));
        assert_eq!(second, ScanId(2));
    }

    #[test]
    fn cancel_scan_flips_only_that_scan() {
        let state = AppState::default();
        let (first, first_flag) = state.begin_scan();
        let (_second, second_flag) = state.begin_scan();

        state.cancel_scan(first);

        assert!(first_flag.load(Ordering::Relaxed));
        assert!(!second_flag.load(Ordering::Relaxed));
    }

    #[test]
    fn cancel_all_flips_every_live_scan() {
        let state = AppState::default();
        let (_, first) = state.begin_scan();
        let (_, second) = state.begin_scan();

        state.cancel_all();

        assert!(first.load(Ordering::Relaxed));
        assert!(second.load(Ordering::Relaxed));
    }

    /// What the watcher and the poll check before doing any work: a scan is about to re-read every
    /// row in the tree, so a refresh underneath it is duplicated I/O.
    #[test]
    fn a_scan_is_in_flight_only_while_it_is_registered() {
        let state = AppState::default();
        assert!(!state.scan_in_flight());

        let (id, _flag) = state.begin_scan();
        assert!(state.scan_in_flight());

        state.finish_scan(id);
        assert!(!state.scan_in_flight());
    }

    /// Cancelling a scan that already finished is a no-op, not a panic: the frontend cannot avoid
    /// that race, so it must not be punished for losing it.
    #[test]
    fn finish_scan_removes_the_entry_and_is_idempotent() {
        let state = AppState::default();
        let (id, _flag) = state.begin_scan();

        state.finish_scan(id);
        state.finish_scan(id);
        state.cancel_scan(id);
    }
}

#[cfg(test)]
mod tier1_tests {
    use repo_scan::{FileCounts, Tier1};

    use super::{tests::row, *};

    fn tier1(path: &str, dirty: bool, conflicted: u32) -> Tier1 {
        Tier1 {
            path: PathBuf::from(path),
            dirty,
            conflicted,
            error: None,
        }
    }

    /// The mirror of the Tier 0 case: Tier 1 owns two fields and must leave every Tier 0 field
    /// exactly as it found it. A wholesale replace is impossible here — Tier 1 has no `head` — but
    /// a merge that recomputed identity or cleared `upstream` would still be wrong.
    #[test]
    fn tier1_merge_leaves_every_tier0_field_alone() {
        let mut existing = row("C:/work/a");
        existing.upstream = Some("origin/main".into());
        existing.ahead = Some(2);
        existing.stash_count = 3;

        let merged = merge_tier1(&existing, tier1("C:/work/a", true, 1));

        assert_eq!(merged.upstream, Some("origin/main".into()));
        assert_eq!(merged.ahead, Some(2));
        assert_eq!(merged.stash_count, 3);
        assert_eq!(merged.head, existing.head);
        assert_eq!(merged.dirty, Some(true));
        assert_eq!(merged.conflicted, Some(1));
    }

    /// Tier 2's fields are not Tier 1's to touch either.
    #[test]
    fn tier1_merge_keeps_a_tier2_value() {
        let mut existing = row("C:/work/a");
        existing.counts = Some(FileCounts::default());

        let merged = merge_tier1(&existing, tier1("C:/work/a", false, 0));

        assert_eq!(merged.counts, Some(FileCounts::default()));
    }

    /// A clean worktree is `Some(false)`, not `None`. The distinction is the whole point: `None`
    /// means "not counted", and rendering that as clean is the bug the tiering exists to prevent.
    #[test]
    fn a_clean_worktree_is_a_computed_false_not_an_unknown() {
        let merged = merge_tier1(&row("C:/work/a"), tier1("C:/work/a", false, 0));

        assert_eq!(merged.dirty, Some(false));
        assert_eq!(merged.conflicted, Some(0));
    }

    /// Both tiers can fail independently and describe different halves of the row, so a Tier 1
    /// failure must not erase Tier 0's reason.
    #[test]
    fn both_tiers_causes_survive_in_the_one_error_slot() {
        let mut existing = row("C:/work/a");
        existing.error = Some("stash count failed".into());
        let mut incoming = tier1("C:/work/a", true, 0);
        incoming.error = Some("index unreadable".into());

        let merged = merge_tier1(&existing, incoming);

        let error = merged.error.expect("both causes are recorded");
        assert!(error.contains("stash count failed"));
        assert!(error.contains("index unreadable"));
    }

    /// Tier 1 cannot insert a row of its own — it has no `head` to give one.
    #[test]
    fn merge_tier1_batch_drops_a_result_for_a_path_with_no_row() {
        let state = AppState::default();

        let merged = state.merge_tier1_batch(vec![tier1("C:/work/ghost", true, 0)]);

        assert!(merged.is_empty());
        assert!(state.snapshot().is_empty());
    }

    #[test]
    fn merge_tier1_batch_returns_exactly_what_the_map_now_holds() {
        let state = AppState::default();
        state.merge_tier0_batch(vec![row("C:/work/a"), row("C:/work/b")]);

        let merged = state.merge_tier1_batch(vec![
            tier1("C:/work/a", true, 0),
            tier1("C:/work/b", false, 0),
        ]);

        assert_eq!(merged.len(), 2);
        let held = state.snapshot();
        for row in &merged {
            assert!(held.contains(row), "returned row is not what the map holds");
        }
    }
}

#[cfg(test)]
mod tier2_tests {
    use repo_scan::{FileCounts, SubmoduleStatus, Tier2};

    use super::{tests::row, *};

    fn counts() -> FileCounts {
        FileCounts {
            staged: 1,
            unstaged: 2,
            untracked: 3,
            conflicted: 4,
        }
    }

    fn tier2(path: &str) -> Tier2 {
        Tier2 {
            path: PathBuf::from(path),
            counts: Some(counts()),
            submodules: Some(Vec::new()),
            error: None,
        }
    }

    /// Tier 2 owns two fields and must leave every earlier tier's alone.
    #[test]
    fn tier2_merge_leaves_the_earlier_tiers_alone() {
        let mut existing = row("C:/work/a");
        existing.upstream = Some("origin/main".into());
        existing.ahead = Some(2);
        existing.dirty = Some(true);
        existing.conflicted = Some(9);

        let merged = merge_tier2(&existing, tier2("C:/work/a"));

        assert_eq!(merged.upstream, Some("origin/main".into()));
        assert_eq!(merged.ahead, Some(2));
        assert_eq!(merged.dirty, Some(true));
        assert_eq!(merged.counts, Some(counts()));
        assert_eq!(merged.submodules, Some(Vec::new()));
    }

    /// **Tier 1 owns `conflicted`, and Tier 2 counted one too.** Writing it here would give one
    /// field two writers reaching it by different routes, which is how the two answers get to
    /// disagree on screen. `counts.conflicted` carries Tier 2's number instead.
    #[test]
    fn tier2_merge_does_not_write_the_tier1_conflicted_field() {
        let mut existing = row("C:/work/a");
        existing.conflicted = Some(9);

        let merged = merge_tier2(&existing, tier2("C:/work/a"));

        assert_eq!(merged.conflicted, Some(9), "still Tier 1's value");
        assert_eq!(merged.counts.expect("counts were merged").conflicted, 4);
    }

    /// A Tier 2 failure is reported to its caller, never onto the row: it runs once per expand, so
    /// appending would stack a message per expand and replacing would erase an earlier tier's
    /// cause.
    #[test]
    fn tier2_merge_never_touches_the_error_slot() {
        let mut existing = row("C:/work/a");
        existing.error = Some("stash count failed".into());
        let mut incoming = tier2("C:/work/a");
        incoming.error = Some("status iterator failed".into());

        let merged = merge_tier2(&existing, incoming);

        assert_eq!(merged.error, Some("stash count failed".into()));
    }

    /// A read that failed leaves the fields unknown rather than zero — a `FileCounts::default()`
    /// here would be four zeros presented as a measurement.
    #[test]
    fn a_failed_tier2_read_merges_as_unknown_not_as_zero() {
        let existing = row("C:/work/a");
        let incoming = Tier2 {
            path: PathBuf::from("C:/work/a"),
            counts: None,
            submodules: None,
            error: Some("index unreadable".into()),
        };

        let merged = merge_tier2(&existing, incoming);

        assert_eq!(merged.counts, None);
        assert_eq!(merged.submodules, None);
    }

    /// An empty submodule list is an answered question; `None` would claim a read is outstanding.
    #[test]
    fn an_empty_submodule_list_is_distinct_from_an_unread_one() {
        let existing = row("C:/work/a");
        let mut incoming = tier2("C:/work/a");
        incoming.submodules = Some(vec![SubmoduleStatus {
            name: "sub".into(),
            path: PathBuf::from("sub"),
            recorded_id: None,
            head_id: None,
        }]);

        let listed = merge_tier2(&existing, incoming)
            .submodules
            .expect("the list was read");
        assert_eq!(listed.len(), 1);
        assert_eq!(
            merge_tier2(&existing, tier2("C:/work/a")).submodules,
            Some(Vec::new())
        );
    }

    /// The exception to tier ownership, and only for the rows named. A watcher re-reads Tiers 0 and
    /// 1, which by the ownership rule leaves `counts` untouched — so a repository that just changed
    /// would show a pre-change count beside a post-change branch, with no age beside it to say so.
    #[test]
    fn invalidate_tier2_clears_the_counts_and_the_submodules() {
        let state = AppState::default();
        state.merge_tier0_batch(vec![row("C:/work/a"), row("C:/work/b")]);
        state.merge_tier2(tier2("C:/work/a"));
        state.merge_tier2(tier2("C:/work/b"));

        state.invalidate_tier2(&[PathBuf::from("C:/work/a")]);

        let cleared = state
            .row(Path::new("C:/work/a"))
            .expect("the row is still here");
        assert_eq!(cleared.counts, None, "the counts are gone");
        assert_eq!(cleared.submodules, None, "and so is the submodule list");
        assert_eq!(
            state
                .row(Path::new("C:/work/b"))
                .expect("b is untouched")
                .counts,
            Some(counts()),
            "only the paths named are invalidated"
        );
    }

    /// It clears Tier 2 and nothing else. Tier 0's and Tier 1's fields belong to the tiers about to
    /// re-read them, and blanking those would put "counting…" on a row for values Rust still holds.
    #[test]
    fn invalidate_tier2_leaves_every_other_tier_alone() {
        let state = AppState::default();
        let mut existing = row("C:/work/a");
        existing.dirty = Some(true);
        existing.conflicted = Some(2);
        state.merge_tier0_batch(vec![existing]);
        state.merge_tier2(tier2("C:/work/a"));

        state.invalidate_tier2(&[PathBuf::from("C:/work/a")]);

        let cleared = state
            .row(Path::new("C:/work/a"))
            .expect("the row is still here");
        assert_eq!(cleared.dirty, Some(true), "Tier 1's flag is not Tier 2's");
        assert_eq!(cleared.conflicted, Some(2));
        assert_eq!(
            cleared.head,
            row("C:/work/a").head,
            "and neither is Tier 0's"
        );
    }

    /// A path with no row has nothing to invalidate, and inventing one would be the absence-is-the-
    /// signal rule broken from the other side.
    #[test]
    fn invalidate_tier2_inserts_nothing_for_an_unknown_path() {
        let state = AppState::default();

        state.invalidate_tier2(&[PathBuf::from("C:/work/ghost")]);

        assert!(state.snapshot().is_empty());
    }

    /// Tier 2 cannot insert a row of its own — it has no `head` to give one.
    #[test]
    fn merge_tier2_drops_a_result_for_a_path_with_no_row() {
        let state = AppState::default();

        assert!(state.merge_tier2(tier2("C:/work/ghost")).is_none());
        assert!(state.snapshot().is_empty());
    }

    /// `has_repo` gates the commands that need a row; `discovered` gates the one that can create
    /// one. The second is a superset, which is what gives a total Tier 0 failure a retry path.
    #[test]
    fn discovered_is_a_superset_of_the_rows() {
        let state = AppState::default();
        let unreadable = repo_scan::DiscoveredRepo {
            path: PathBuf::from("C:/work/broken"),
            name: "broken".into(),
            parent: PathBuf::from("C:/work"),
            kind: repo_scan::RepoKind::Normal,
            git_dir: PathBuf::from("C:/work/broken/.git"),
            common_dir: PathBuf::from("C:/work/broken/.git"),
        };
        state.record_found(&[unreadable]);

        assert!(
            !state.has_repo(Path::new("C:/work/broken")),
            "no row exists"
        );
        assert!(
            state.discovered(Path::new("C:/work/broken")).is_some(),
            "but the walk found it, so a refresh can retry it"
        );
        assert_eq!(
            state
                .discovered(Path::new("C:/work/broken"))
                .expect("found")
                .git_dir,
            PathBuf::from("C:/work/broken/.git"),
            "and the resolved git dir is what a per-row read needs"
        );
    }

    /// A removed root must not leave `found` entries behind: they are what lets a command reach
    /// into a tree the user has taken away.
    #[test]
    fn remove_root_evicts_the_discovered_map_too() {
        let state = AppState::default();
        state.add_root(PathBuf::from("C:/work"));
        state.record_found(&[repo_scan::DiscoveredRepo {
            path: PathBuf::from("C:/work/a"),
            name: "a".into(),
            parent: PathBuf::from("C:/work"),
            kind: repo_scan::RepoKind::Normal,
            git_dir: PathBuf::from("C:/work/a/.git"),
            common_dir: PathBuf::from("C:/work/a/.git"),
        }]);
        state.merge_tier0_batch(vec![row("C:/work/a")]);

        state.remove_root(Path::new("C:/work"));

        assert!(state.discovered(Path::new("C:/work/a")).is_none());
        assert!(!state.has_repo(Path::new("C:/work/a")));
    }
}

#[cfg(test)]
mod cache_tests {
    use super::{tests::row, *};

    /// The cache paints at launch and its rows must carry their original ages, or a week-old
    /// answer reads as this morning's.
    #[test]
    fn restore_seeds_both_maps_without_touching_the_ages() {
        let state = AppState::default();
        let mut cached = row("C:/work/a");
        cached.scanned_at_ms = 42;

        state.restore(
            vec![cached],
            vec![repo_scan::DiscoveredRepo {
                path: PathBuf::from("C:/work/a"),
                name: "a".into(),
                parent: PathBuf::from("C:/work"),
                kind: repo_scan::RepoKind::Normal,
                git_dir: PathBuf::from("C:/work/a/.git"),
                common_dir: PathBuf::from("C:/work/a/.git"),
            }],
        );

        assert_eq!(
            state
                .row(Path::new("C:/work/a"))
                .expect("restored")
                .scanned_at_ms,
            42,
            "the age is the claim, and nothing here refreshes it"
        );
        assert!(
            state.discovered(Path::new("C:/work/a")).is_some(),
            "and the git dir came back too, or the row could not be refreshed or expanded"
        );
    }

    /// A repository that was deleted between sessions has a cached row and no folder. A completed
    /// scan is the only thing that can know that, and this is how the row goes away.
    #[test]
    fn retain_scanned_evicts_a_row_the_scan_did_not_see() {
        let state = AppState::default();
        state.add_root(PathBuf::from("C:/work"));
        state.merge_tier0_batch(vec![row("C:/work/gone"), row("C:/work/still-here")]);

        let seen = HashSet::from([PathBuf::from("C:/work/still-here")]);
        let evicted = state.retain_scanned(&[PathBuf::from("C:/work")], &seen);

        assert_eq!(evicted, vec![PathBuf::from("C:/work/gone")]);
        assert!(!state.has_repo(Path::new("C:/work/gone")));
        assert!(state.has_repo(Path::new("C:/work/still-here")));
    }

    /// Scanning one root must not evict another's rows. The scan saw nothing under `C:/other`
    /// because it never looked there.
    #[test]
    fn retain_scanned_leaves_rows_under_a_root_it_did_not_walk() {
        let state = AppState::default();
        state.merge_tier0_batch(vec![row("C:/work/a"), row("C:/other/b")]);

        let evicted = state.retain_scanned(
            &[PathBuf::from("C:/work")],
            &HashSet::from([PathBuf::from("C:/work/a")]),
        );

        assert!(evicted.is_empty());
        assert!(state.has_repo(Path::new("C:/other/b")));
    }

    /// The discovered map is evicted on the same terms, for the reason `remove_root` evicts it:
    /// otherwise a gone repository keeps the entry that lets a command reach into it.
    #[test]
    fn retain_scanned_evicts_the_discovered_map_too() {
        let state = AppState::default();
        state.record_found(&[repo_scan::DiscoveredRepo {
            path: PathBuf::from("C:/work/gone"),
            name: "gone".into(),
            parent: PathBuf::from("C:/work"),
            kind: repo_scan::RepoKind::Normal,
            git_dir: PathBuf::from("C:/work/gone/.git"),
            common_dir: PathBuf::from("C:/work/gone/.git"),
        }]);

        state.retain_scanned(&[PathBuf::from("C:/work")], &HashSet::new());

        assert!(state.discovered(Path::new("C:/work/gone")).is_none());
    }
}
