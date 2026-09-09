//! The canonical row state, the configured roots, and the live scans.
//!
//! Rust owns the one `HashMap<PathBuf, RepoStatus>`. Each tier result is merged into it here and
//! the **full merged row** is what goes over the channel; the Pinia store is a mirror that never
//! merges and never holds a value this map does not. That is the whole reason the merge lives on
//! this side: a tiered stream delivers results out of order, and a frontend that tried to
//! reconcile them would eventually show a stale value as a fresh one.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use repo_scan::{RepoEvent, RepoStatus, ScanError, ScanId, Tier1};
use tauri::ipc::Channel;

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
    /// `RwLock` and not `Mutex` because path validation and the Phase 5 cache snapshot are reads.
    /// Not `dashmap`: the write side takes the lock once per batch rather than once per row, so
    /// per-key sharding buys nothing here — and a sharded map cannot cheaply produce the coherent
    /// snapshot the cache needs.
    repos: RwLock<HashMap<PathBuf, RepoStatus>>,

    /// The configured roots, canonicalised. In memory only; persistence is Phase 5.
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
}

/// Hand-written because `tauri::ipc::Channel` implements no `Debug`, and because dumping every row
/// would make the derived output useless anyway. Reports sizes, which is what a log line wants.
impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppState")
            .field("repos", &self.read_repos().len())
            .field("roots", &self.read_roots().len())
            .field("scans", &self.lock_scans().len())
            .field("session", &self.lock_session().is_some())
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

        let mut repos = self.write_repos();
        let evicted: Vec<PathBuf> = repos
            .keys()
            .filter(|key| key.starts_with(path))
            .cloned()
            .collect();
        for key in &evicted {
            repos.remove(key);
        }
        (roots, evicted)
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

        // Owned by the read that produced it. Unambiguous while Tier 0 is the only writer; when
        // Phase 4 adds a second one, how two tiers' causes combine is an open question — see
        // PLAN.md §12.
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
