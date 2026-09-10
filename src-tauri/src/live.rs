//! Live updates: the watcher, the poll, and the refresh-on-focus.
//!
//! Three triggers, one operation. A change notice never crosses the IPC boundary as "something
//! happened" — Rust re-reads the repository, merges the result into the canonical map, and pushes
//! the full merged row on the session channel. The frontend has nothing to do but render what
//! arrives, which is why almost none of this phase is on that side of the boundary.
//!
//! # Two threads, and what each one owns
//!
//! **The refresh thread** drains the watcher. The debouncer's callback does nothing but `send`,
//! because a callback that blocks lets OS events pile up in the kernel buffer until they are
//! dropped on overflow — so the work happens here instead, batched through the same
//! [`crate::stream::next_batch`] the scan pipeline uses. It re-reads Tiers 0 **and** 1, because a
//! watcher event is evidence that something changed and the dirty flag is the field most likely to
//! have changed with it.
//!
//! **The poll thread** is the safety net, and it owns **Tier 0 only**. `notify`'s own docs warn
//! that a backend "may fail to receive all events" and is "not a 100% reliable source", so a
//! low-frequency pass runs regardless. It is parked on `recv_timeout`, which makes the poll and the
//! focus refresh literally the same code path: the timeout is the poll, a message is the focus
//! refresh, and a second message is shutdown. Two timers would be two paths to keep in step.
//!
//! # Which tiers each trigger claims
//!
//! Tier ownership decides this, exactly as it does in a scan. The watcher re-reads Tiers 0 and 1
//! and additionally **invalidates Tier 2**, because those counts describe a worktree that has just
//! changed and the drawer shows them with no age beside them. The poll claims Tier 0 and nothing
//! else — it has no evidence that anything changed, only that time passed, so nulling a count on a
//! timer would manufacture a `counting…` flicker every minute for a drawer nobody touched. That the
//! poll leaves `dirty` alone and leaves `counts` alone is the same decision, applied consistently.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::AtomicBool,
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow};
use repo_scan::{
    DiscoveredRepo, RepoEvent, RepoStatus, RepoWatcher, Tier, WatchEvent, read_tier0,
    read_tier0_all_with, read_tier1, read_tier2,
};

use crate::{
    persist::WatchSettings,
    state::AppState,
    stream::{BATCH_MAX, BATCH_WINDOW, next_batch},
};

/// How soon after refreshing one repository another refresh of it is allowed.
///
/// The debouncer already collapses the create/modify/remove burst that one `git` write produces.
/// This covers the case it cannot: a sequence of separate operations — `git add`, then `commit`,
/// then `push` — each of which is a genuinely distinct burst arriving a second apart. §7.3 asks for
/// both, and they are not the same guard.
const COOLDOWN: Duration = Duration::from_millis(750);

/// The shortest gap between two full poll passes.
///
/// The focus refresh shares the poll's path, and window focus is not a rare event — alt-tabbing
/// between an editor and this window would otherwise run a Tier 0 pass over the whole tree every
/// couple of seconds.
const POLL_FLOOR: Duration = Duration::from_secs(5);

/// What wakes the poll thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Run a pass now, subject to [`POLL_FLOOR`]. Sent by the focus handler.
    Refresh,
    /// Stop the thread. Sent on window close.
    Stop,
}

/// One thing the watcher had to say, on its way to the refresh thread.
///
/// A single channel for both, so the debouncer's callback has exactly one thing it can do and
/// cannot accidentally grow a blocking branch.
#[derive(Debug)]
enum Notice {
    /// This repository changed.
    Changed(PathBuf),
    /// Watching failed or degraded.
    Failed(String),
}

/// Start live updates. Called once, from `setup`.
///
/// The poll thread starts **whatever happens to the watcher**, and that ordering is the point: a
/// watcher that cannot start is exactly when the fallback matters, so it must not be downstream of
/// the watcher's success. A failure to build one is pushed to the user and the app carries on
/// polling.
///
/// Registers nothing yet. The watch set follows from what discovery finds, so the first
/// registration happens when the launch scan completes — see [`AppState::sync_watches`].
pub fn start(state: &Arc<AppState>, settings: &WatchSettings) {
    let (tick_tx, tick_rx) = mpsc::channel::<Tick>();
    state.set_poll(tick_tx);

    let interval = settings.poll_interval();
    let polling = Arc::clone(state);
    std::thread::Builder::new()
        .name("repo-poll".to_string())
        .spawn(move || poll_loop(&polling, &tick_rx, interval))
        .map_or_else(
            |error| tracing::error!(%error, "could not start the poll thread"),
            |_| tracing::debug!(?interval, "polling"),
        );

    if !settings.enabled {
        tracing::info!("watching is disabled in settings.json; the poll is the only refresh");
        return;
    }

    let (notice_tx, notice_rx) = mpsc::channel::<Notice>();
    let refreshing = Arc::clone(state);
    if let Err(error) = std::thread::Builder::new()
        .name("repo-refresh".to_string())
        .spawn(move || refresh_loop(&refreshing, &notice_rx))
    {
        tracing::error!(%error, "could not start the refresh thread; not watching");
        return;
    }

    // The callback runs on the debouncer's own thread and must never block: everything it can do is
    // a channel send, and the sender's `Drop` — when the debouncer is dropped on window close — is
    // what ends the refresh thread, so there is no second shutdown signal to get wrong.
    let watcher = RepoWatcher::new(settings.debounce(), move |event| {
        let sent = match event {
            WatchEvent::Changed(paths) => paths
                .into_iter()
                .try_for_each(|path| notice_tx.send(Notice::Changed(path))),
            WatchEvent::Failed(message) => notice_tx.send(Notice::Failed(message)),
        };
        if sent.is_err() {
            // The refresh thread is gone, which happens on shutdown. Nothing to report.
        }
    });

    match watcher {
        Ok(watcher) => state.set_watcher(watcher),
        Err(error) => {
            let message = error.to_string();
            tracing::warn!(%message, "could not start the watcher");
            state.push(RepoEvent::WatchFailed { message });
        }
    }
}

/// Re-read one repository up to `tier`, merging each tier as it completes.
///
/// The one per-repository refresh in the app. `refresh_repo` answers an invocation with it, the
/// watcher pushes what it returns, and the poll uses its Tier 0 half through the fan-out — which is
/// what keeps a watcher refresh and a user-requested one from ever disagreeing about what a refresh
/// is.
///
/// Cumulative, because the tiers are not independent: Tier 1's `dirty` describes a worktree
/// relative to the `head` Tier 0 reads, so refreshing one without the other would pair a fresh flag
/// with a stale ref. `Tier::Two` therefore means all three.
///
/// Each tier is merged separately rather than assembled and merged once, so this shares the
/// pipeline's merge functions exactly — and so a Tier 1 failure still leaves Tier 0's fresh values
/// in the map.
pub fn refresh_one(
    state: &AppState,
    found: &DiscoveredRepo,
    tier: Tier,
) -> anyhow::Result<RepoStatus> {
    let flag = Arc::new(AtomicBool::new(false));

    // Tier 0 always runs: it is the tier that produces the row at all, and the only one whose
    // failure means there is no honest row to return.
    let row = read_tier0(found).context("could not read the repository's refs")?;
    let mut merged = state
        .merge_tier0_batch(vec![row])
        .pop()
        .ok_or_else(|| anyhow!("`{}` was read but produced no row", found.path.display()))?;

    if tier >= Tier::One
        && let Some(dirty) = read_tier1(found, &flag).context("could not read the worktree")?
    {
        merged = state.merge_tier1_batch(vec![dirty]).pop().unwrap_or(merged);
    }

    if tier >= Tier::Two
        && let Some(counts) = read_tier2(found, &flag).context("could not read the file counts")?
    {
        merged = state.merge_tier2(counts).unwrap_or(merged);
    }

    Ok(merged)
}

/// Drain the watcher until its sender is dropped.
///
/// Batched rather than one refresh per notice, for the reason the scan pipeline batches: the
/// channel send is the expensive part, and a `git pull` across a handful of repositories arrives as
/// a handful of notices inside one debounce window.
fn refresh_loop(state: &Arc<AppState>, notices: &Receiver<Notice>) {
    let mut cooldown = Cooldown::new(COOLDOWN);

    while let Some(batch) = next_batch(notices, BATCH_MAX, BATCH_WINDOW) {
        let mut changed: Vec<PathBuf> = Vec::with_capacity(batch.len());
        let mut failures: Vec<String> = Vec::new();
        for notice in batch {
            match notice {
                Notice::Changed(path) => changed.push(path),
                // Deduplicated because a watch limit reports the same cause per path, and a user
                // needs to read it once.
                Notice::Failed(message) if !failures.contains(&message) => failures.push(message),
                Notice::Failed(_) => {}
            }
        }

        for message in failures {
            tracing::warn!(%message, "watching degraded");
            state.push(RepoEvent::WatchFailed { message });
        }

        // A scan is about to re-read the whole tree, so refreshing a row underneath it is
        // duplicated I/O that puts a second copy of the same answer on a second channel. Dropping
        // the notice is safe rather than lossy: if the scan had already passed this repository, the
        // poll picks the change up.
        if changed.is_empty() || state.scan_in_flight() {
            continue;
        }

        let now = Instant::now();
        let due: Vec<PathBuf> = changed
            .into_iter()
            .filter(|path| cooldown.due(path, now))
            .collect();
        if due.is_empty() {
            continue;
        }

        // Before the re-read, so the merged rows that go out already carry the cleared fields and
        // one push says everything. See `AppState::invalidate_tier2` for why this is the watcher's
        // job and not the merge's.
        state.invalidate_tier2(&due);

        let mut rows: Vec<RepoStatus> = Vec::with_capacity(due.len());
        for path in &due {
            // Gone between the event and now — evicted by a scan, or its root removed. Not an
            // error: the row it would have refreshed is gone too.
            let Some(found) = state.discovered(path) else {
                continue;
            };
            match refresh_one(state, &found, Tier::One) {
                Ok(row) => rows.push(row),
                // A repository that will not read has no honest row, and the one it already has
                // keeps its `scanned_at` age showing — which is the same thing a scan does. There
                // is no invocation waiting on this, so there is nowhere to report it but the log.
                Err(error) => {
                    tracing::debug!(path = %path.display(), %error, "watched refresh failed");
                }
            }
        }

        if !rows.is_empty() {
            tracing::debug!(count = rows.len(), "pushed watched rows");
            state.push(RepoEvent::Updated { repos: rows });
        }
    }

    tracing::debug!("refresh thread ended");
}

/// Wait, then run a Tier 0 pass — on the timer, or when something asks.
fn poll_loop(state: &Arc<AppState>, ticks: &Receiver<Tick>, interval: Duration) {
    // Seeded as though a pass had just run, because one effectively has: a launch reconciles, and
    // polling the tree a second time while that scan is still walking it would be pure waste.
    let mut last = Instant::now();

    loop {
        match ticks.recv_timeout(interval) {
            // Window close, or the app going away with the sender.
            Ok(Tick::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Tick::Refresh) if last.elapsed() < POLL_FLOOR => continue,
            Ok(Tick::Refresh) | Err(RecvTimeoutError::Timeout) => {}
        }

        if state.scan_in_flight() {
            continue;
        }
        last = Instant::now();
        pass(state);
    }

    tracing::debug!("poll thread ended");
}

/// One Tier 0 pass over every repository discovery has found.
///
/// **Tier 0 only.** It is refs-only and cheap enough to run unprompted — roughly 2.6 ms per
/// repository warm — where Tier 1 is an order of magnitude more and would put a worktree walk over
/// the whole tree on a timer. The rows that come back are merged and pushed in batches, exactly as
/// a scan's are.
fn pass(state: &AppState) {
    let found = state.discovered_all();
    if found.is_empty() {
        return;
    }

    // Its own flag, never a shared one. Nothing cancels a poll pass — it is short, and the thread
    // that would cancel it is the one running it — so this exists to satisfy the signature.
    let flag = AtomicBool::new(false);
    let rows = Mutex::new(Vec::with_capacity(found.len()));
    let summary = read_tier0_all_with(&found, &flag, |row| lock(&rows).push(row));

    let read = std::mem::take(&mut *lock(&rows));
    let merged = state.merge_tier0_batch(read);
    tracing::debug!(
        repos = merged.len(),
        errors = summary.errors.len(),
        elapsed_ms = summary.elapsed_ms,
        "poll pass"
    );

    for chunk in merged.chunks(BATCH_MAX) {
        state.push(RepoEvent::Updated {
            repos: chunk.to_vec(),
        });
    }

    // A repository that stopped reading between scans. The row keeps everything it has and gains
    // the cause, which is what `record_errors` is for.
    if !summary.errors.is_empty() {
        let flagged = state.record_errors(&summary.errors);
        for chunk in flagged.chunks(BATCH_MAX) {
            state.push(RepoEvent::Updated {
                repos: chunk.to_vec(),
            });
        }
    }
}

/// Per-repository refresh rate limiting.
///
/// Split out and pure — `due` takes the clock rather than reading it — so §7.3's cooldown is
/// testable without a watcher, a thread, or a sleep.
struct Cooldown {
    /// The minimum gap between two refreshes of one repository.
    window: Duration,
    /// When each repository was last refreshed. Bounded by the size of the watch set.
    last: HashMap<PathBuf, Instant>,
}

impl Cooldown {
    /// A cooldown with nothing recorded.
    fn new(window: Duration) -> Self {
        Self {
            window,
            last: HashMap::new(),
        }
    }

    /// Whether `path` may be refreshed at `now`, recording it if so.
    ///
    /// A first sighting is always due: the cooldown throttles repeats, and treating an unknown path
    /// as too soon would drop the very first event for every repository.
    fn due(&mut self, path: &Path, now: Instant) -> bool {
        if let Some(last) = self.last.get(path)
            && now.duration_since(*last) < self.window
        {
            return false;
        }
        self.last.insert(path.to_path_buf(), now);
        true
    }
}

/// Lock through a poison rather than cascading one panic into the refresh thread.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard §7.3 asks for: a second event inside the window is dropped.
    #[test]
    fn a_repeat_inside_the_window_is_not_due() {
        let mut cooldown = Cooldown::new(Duration::from_millis(500));
        let start = Instant::now();
        let path = Path::new("C:/repos/one");

        assert!(cooldown.due(path, start), "a first sighting is always due");
        assert!(
            !cooldown.due(path, start + Duration::from_millis(100)),
            "100 ms later is inside the 500 ms window"
        );
    }

    /// And one past it is allowed, which is what keeps `git add` then `commit` from collapsing into
    /// a single refresh that misses the commit.
    #[test]
    fn a_repeat_past_the_window_is_due_again() {
        let mut cooldown = Cooldown::new(Duration::from_millis(500));
        let start = Instant::now();
        let path = Path::new("C:/repos/one");

        assert!(cooldown.due(path, start));
        assert!(cooldown.due(path, start + Duration::from_millis(600)));
    }

    /// Per repository, not global. One busy repository must not silence its neighbours.
    #[test]
    fn the_cooldown_is_per_repository() {
        let mut cooldown = Cooldown::new(Duration::from_secs(10));
        let now = Instant::now();

        assert!(cooldown.due(Path::new("C:/repos/one"), now));
        assert!(
            cooldown.due(Path::new("C:/repos/two"), now),
            "a different repository has its own window"
        );
    }

    /// The window is measured from the last refresh, not from the first — so a steady stream of
    /// events refreshes once per window rather than once and never again.
    #[test]
    fn the_window_restarts_from_the_last_refresh() {
        let mut cooldown = Cooldown::new(Duration::from_millis(500));
        let start = Instant::now();
        let path = Path::new("C:/repos/one");

        assert!(cooldown.due(path, start));
        assert!(cooldown.due(path, start + Duration::from_millis(500)));
        assert!(
            !cooldown.due(path, start + Duration::from_millis(700)),
            "200 ms after the second refresh is still inside the window"
        );
    }
}
