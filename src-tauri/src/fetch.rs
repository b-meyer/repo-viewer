//! The fetch driver: the engine's pool on one side, the canonical map and two channels on the
//! other.
//!
//! Structurally [`crate::pipeline`], and deliberately so — a guard that guarantees one terminal
//! event, a worker thread, and a [`crate::stream::next_batch`] drain. Nothing here is `async` and
//! nothing needs an `AppHandle`, which is what lets the whole driver be exercised against a real
//! `tauri::ipc::Channel` with no Tauri application.
//!
//! # Two channels, and what goes on each
//!
//! Outcomes go out on this invocation's `Channel<FetchEvent>`. The refreshed **rows** go out on
//! the session `Channel<RepoEvent>`, because Rust owns the canonical copy of a row and a second
//! shape carrying one would be a second source of truth for it. A consumer that only wants the
//! table updated can ignore the fetch channel entirely.
//!
//! # Where the work happens, and why it is split that way
//!
//! The engine's workers do the fetching. The **re-read happens on this thread**, in batches: four
//! workers each merging and pushing one row would take the session mutex four times per window
//! and defeat the batching the channel exists for. Same division `pipeline.rs` makes between the
//! walker thread and its drain loop.
//!
//! # Suppression starts on the worker, not here
//!
//! `AppState::begin_fetch_group` is called from the `Started` callback, on the worker, *before*
//! the process is spawned. Routing it through the channel first would leave a whole
//! `BATCH_WINDOW` in which the fetch has already written `FETCH_HEAD` and the debouncer is
//! already counting. We would win that race almost always — and "almost always" is how this class
//! of bug ships.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::{Duration, Instant},
};

use repo_scan::{
    DiscoveredRepo, FetchEvent, FetchId, FetchNotice, FetchOpts, FetchOutcome, FetchSummary,
    RepoEvent, RepoStatus, Tier, fetch_all_with,
};
use tauri::ipc::Channel;

use crate::{
    live::refresh_one,
    state::AppState,
    stream::{BATCH_MAX, BATCH_WINDOW, next_batch},
};

/// Run one fetch pass to completion on the calling thread.
///
/// Returns when every repository has settled or `cancel` is flipped. Either way the pass is
/// deregistered, every suppression it took is released, and exactly one
/// [`FetchEvent::Finished`] reaches the frontend.
pub fn run_fetch(
    state: Arc<AppState>,
    id: FetchId,
    repos: Vec<DiscoveredRepo>,
    opts: FetchOpts,
    tail: Duration,
    cancel: Arc<AtomicBool>,
    events: Channel<FetchEvent>,
) {
    let started = Instant::now();
    let paths: Vec<PathBuf> = repos.iter().map(|repo| repo.path.clone()).collect();
    let mut guard = FetchGuard::new(Arc::clone(&state), id, events.clone(), paths.clone());

    emit(&events, FetchEvent::Queued { paths });

    let (done_tx, done_rx) = mpsc::channel::<FetchNotice>();

    // The pool runs on its own thread so this one can drain, re-read and push while it works.
    // `done_tx` is moved into the closure and dropped when `fetch_all_with` returns, and that
    // disconnect is what ends the drain loop — no separate "the pass is done" signal to get wrong.
    let worker = std::thread::spawn({
        let (state, opts, cancel) = (Arc::clone(&state), opts.clone(), Arc::clone(&cancel));
        let hold = opts.timeout + tail;
        move || {
            fetch_all_with(&repos, &opts, &cancel, move |notice| {
                // Before the spawn, on the worker. See the module doc.
                if let FetchNotice::Started(path) = &notice
                    && let Some(found) = state.discovered(path)
                {
                    let group: Vec<PathBuf> = state
                        .siblings(&found)
                        .into_iter()
                        .map(|repo| repo.path)
                        .collect();
                    state.begin_fetch_group(&group, hold);
                }
                let _ = done_tx.send(notice);
            })
        }
    });

    while let Some(batch) = next_batch(&done_rx, BATCH_MAX, BATCH_WINDOW) {
        let mut started_now: Vec<PathBuf> = Vec::new();
        let mut results: Vec<FetchOutcome> = Vec::new();
        for notice in batch {
            match notice {
                FetchNotice::Started(path) => started_now.push(path),
                FetchNotice::Done(result) => results.push(result),
            }
        }

        if !started_now.is_empty() {
            emit(&events, FetchEvent::Fetching { paths: started_now });
        }
        if results.is_empty() {
            continue;
        }

        let rows = settle(&state, &results, tail);
        if !rows.is_empty() {
            state.push(RepoEvent::Updated { repos: rows });
        }
        guard.settled += u32::try_from(results.len()).unwrap_or(u32::MAX);
        emit(&events, FetchEvent::Results { results });
    }

    let summary = match worker.join() {
        Ok(summary) => summary,
        // The pool panicked. The guard still emits a terminal event from `Drop`, so the frontend
        // is not left spinning; this only decides what the summary says.
        Err(_) => FetchSummary {
            cancelled: true,
            elapsed_ms: elapsed_ms(started),
            ..FetchSummary::default()
        },
    };
    guard.finish(summary);
}

/// Re-read and release the repositories in one batch of results, and return the merged rows.
///
/// # Tier 1, not Tier 0, and not a `last_fetched_ms` patch
///
/// A fetch moves `behind`, can move `ahead`, moves the tracking ref's tip, and with `--prune` can
/// remove `upstream` outright — so writing one field would leave four stale beside it, which is
/// §8.2's own premise inverted. Tier **1** rather than Tier 0 because this fetch suppressed the
/// watcher for these repositories, and the watcher's notice is what would otherwise have caught a
/// `git add` made while the fetch ran. ~17 ms per repository is noise beside a network fetch.
///
/// # Tier 2 only when a notice was deferred
///
/// A fetch writes nothing Tier 2 measures — its inputs are the HEAD tree, the index and the
/// worktree, and a fetch touches none of them — so invalidating unconditionally would put a false
/// `counting…` in a drawer nothing disturbed. But the notice this fetch suppressed might have been
/// the user's own `git add`, so `end_fetch_group` reports which repositories saw one and those are
/// invalidated here, exactly as `refresh_loop` does. The watcher's evidence is still the only
/// thing that invalidates.
fn settle(state: &AppState, results: &[FetchOutcome], tail: Duration) -> Vec<RepoStatus> {
    let mut group: Vec<DiscoveredRepo> = Vec::new();

    for result in results {
        if !result.status.ran() {
            // No process, so no refs moved and nothing to re-read. Still released below, because
            // `Started` took the suppression before the pre-flight answered.
            continue;
        }
        // Re-checked here rather than trusted from the start of the pass. A fetch takes seconds,
        // and `remove_root` or a completing scan's eviction can land inside that window —
        // `merge_tier0` inserts where there was nothing, so merging a result for an evicted path
        // would resurrect a row under no configured root that nothing could ever evict again.
        let Some(found) = state.discovered(&result.path) else {
            continue;
        };
        for sibling in state.siblings(&found) {
            if !group.iter().any(|repo| repo.path == sibling.path) {
                group.push(sibling);
            }
        }
    }

    let paths: Vec<PathBuf> = group.iter().map(|repo| repo.path.clone()).collect();
    let deferred = state.end_fetch_group(&paths, tail);
    if !deferred.is_empty() {
        state.invalidate_tier2(&deferred);
    }

    let mut rows = Vec::with_capacity(group.len());
    for found in &group {
        match refresh_one(state, found, Tier::One) {
            Ok(row) => rows.push(row),
            // The row keeps its last-known values and its `scanned_at` age, which is what a scan
            // does with a repository it cannot read. The fetch's own outcome already went to the
            // caller, so there is nowhere else this belongs.
            Err(error) => {
                tracing::debug!(path = %found.path.display(), %error, "post-fetch refresh failed");
            }
        }
    }
    rows
}

/// Deregisters a fetch and guarantees exactly one terminal event.
///
/// The terminal event is emitted from `Drop`, so it survives an unwind — without it a panic
/// anywhere in the driver would leave a registry entry, a spinner running forever, and every
/// suppression this pass took still in force. Nobody would see any of it, because nothing awaits
/// the `spawn_blocking` handle.
///
/// The sweep is the third of the three layers under the suppression map, and the weakest by
/// design: the per-entry deadline is what actually guarantees nothing leaks, and this only makes
/// the common failure fast. `panic = "unwind"` is pinned in the root profile, which is what makes
/// an RAII guard reliable here at all.
struct FetchGuard {
    state: Arc<AppState>,
    id: FetchId,
    events: Channel<FetchEvent>,
    /// Everything this pass took suppression for, so `Drop` can release all of it.
    paths: Vec<PathBuf>,
    /// Repositories reported settled so far, for the summary a panic would otherwise not have.
    settled: u32,
    /// Set once a terminal event has been emitted, so `Drop` does not send a second.
    finished: bool,
}

impl FetchGuard {
    fn new(
        state: Arc<AppState>,
        id: FetchId,
        events: Channel<FetchEvent>,
        paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            state,
            id,
            events,
            paths,
            settled: 0,
            finished: false,
        }
    }

    /// Emit [`FetchEvent::Finished`]. The pass ran to the end of its list.
    fn finish(&mut self, summary: FetchSummary) {
        self.finished = true;
        emit(&self.events, FetchEvent::Finished { summary });
    }
}

impl Drop for FetchGuard {
    fn drop(&mut self) {
        if !self.finished {
            emit(
                &self.events,
                FetchEvent::Finished {
                    summary: FetchSummary {
                        skipped: self.settled,
                        cancelled: true,
                        ..FetchSummary::default()
                    },
                },
            );
        }
        // Outright rather than with a tail: there is no fetch left whose writes a tail would be
        // absorbing, and a pass that unwound may never have spawned anything at all.
        self.state.sweep_fetching(&self.paths);
        self.state.finish_fetch(self.id);
    }
}

/// Send one event, logging a failure rather than acting on it.
///
/// `send`'s `Err` is not a liveness signal: it returns `Ok(())` for a closed webview and only
/// fails once the app is shutting down, so letting it stop the pass would abandon a fetch for the
/// one condition where stopping changes nothing.
fn emit(channel: &Channel<FetchEvent>, event: FetchEvent) {
    if let Err(error) = channel.send(event) {
        tracing::warn!(%error, "could not send a fetch event");
    }
}

/// Milliseconds since `started`, saturating.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        sync::{Mutex, PoisonError},
    };

    use repo_scan::{FetchStatus, RepoKind, ScanOpts, discover_roots};

    use super::*;

    /// Collect events from a **real** `Channel<FetchEvent>`.
    ///
    /// `Channel::new` needs no `AppHandle`, so these assertions run over what actually crosses
    /// the wire rather than over a mock's idea of it — the same reason `pipeline.rs` has no
    /// `EventSink` trait.
    fn collecting_channel() -> (Channel<FetchEvent>, Arc<Mutex<Vec<FetchEvent>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let channel = Channel::new(move |body| {
            let json = body.deserialize::<FetchEvent>().expect("a FetchEvent body");
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(json);
            Ok(())
        });
        (channel, seen)
    }

    /// A throwaway origin and one clone of it.
    ///
    /// Built here rather than fetched from anywhere real: fetching **this** repository would hit
    /// the network, and this crate is read-only apart from the fetch it is testing.
    fn tree() -> (tempfile::TempDir, DiscoveredRepo) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path().to_path_buf();

        let git = |cwd: &std::path::Path, args: &[&str]| {
            let output = Command::new("git")
                .current_dir(cwd)
                .args([
                    "-c",
                    "init.defaultBranch=main",
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                ])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", cwd.join("no-such-gitconfig"))
                .env("GIT_CONFIG_SYSTEM", cwd.join("no-such-gitconfig"))
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .expect("git is on PATH");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };

        git(&root, &["init", "--bare", "origin.git"]);
        let origin = root.join("origin.git").to_string_lossy().replace('\\', "/");
        git(&root, &["clone", &origin, "seed"]);
        let seed = root.join("seed");
        std::fs::write(seed.join("a.txt"), "one\n").expect("write");
        git(&seed, &["add", "a.txt"]);
        git(&seed, &["commit", "-m", "seed"]);
        git(&seed, &["push", "-u", "origin", "main"]);
        git(&root, &["clone", &origin, "alpha"]);

        // Through discovery rather than by joining paths, so the repository carries the
        // canonicalised key the rest of the app uses — a temp directory on Windows sits behind a
        // short name, and a hand-built path would compare unequal to everything.
        let (found, _) = discover_roots(
            &[root.join("alpha")],
            &ScanOpts {
                max_depth: Some(2),
                ..ScanOpts::default()
            },
            &AtomicBool::new(false),
        );
        let repo = found
            .into_iter()
            .find(|repo| repo.name == "alpha")
            .expect("the clone is discovered");
        assert_eq!(repo.kind, RepoKind::Normal);

        (dir, repo)
    }

    fn state_with(repo: &DiscoveredRepo) -> Arc<AppState> {
        let state = Arc::new(AppState::default());
        state.record_found(std::slice::from_ref(repo));
        state
    }

    fn kinds(events: &[FetchEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event {
                FetchEvent::Queued { .. } => "queued",
                FetchEvent::Fetching { .. } => "fetching",
                FetchEvent::Results { .. } => "results",
                FetchEvent::Finished { .. } => "finished",
            })
            .collect()
    }

    /// The whole driver, against a real channel and a real `git`.
    #[test]
    fn a_fetch_streams_its_outcomes_and_pushes_the_refreshed_row() {
        let (_dir, repo) = tree();
        let state = state_with(&repo);
        let (channel, seen) = collecting_channel();
        let (id, cancel) = state.begin_fetch();

        run_fetch(
            Arc::clone(&state),
            id,
            vec![repo.clone()],
            FetchOpts::default(),
            Duration::from_millis(50),
            cancel,
            channel,
        );

        let events = seen.lock().expect("not poisoned").clone();
        let kinds = kinds(&events);
        assert_eq!(kinds.first(), Some(&"queued"), "got {kinds:?}");
        assert_eq!(kinds.last(), Some(&"finished"), "got {kinds:?}");
        assert_eq!(
            kinds.iter().filter(|kind| **kind == "finished").count(),
            1,
            "exactly one terminal event"
        );

        let results: Vec<&FetchOutcome> = events
            .iter()
            .filter_map(|event| match event {
                FetchEvent::Results { results } => Some(results),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].status,
            FetchStatus::Ok,
            "{:?}",
            results[0].detail
        );

        // The row is the deliverable: Rust re-read it and merged it, so the map has one now.
        let row = state.row(&repo.path).expect("the fetch produced a row");
        assert!(
            row.last_fetched_ms.is_some(),
            "the age the table renders comes from this re-read"
        );
    }

    /// The pass releases every suppression it took, so the watcher resumes.
    #[test]
    fn a_finished_fetch_leaves_nothing_suppressed() {
        let (_dir, repo) = tree();
        let state = state_with(&repo);
        let (channel, _seen) = collecting_channel();
        let (id, cancel) = state.begin_fetch();

        run_fetch(
            Arc::clone(&state),
            id,
            vec![repo],
            FetchOpts::default(),
            Duration::ZERO,
            cancel,
            channel,
        );

        assert!(
            !state.fetch_in_flight(),
            "a released pass must not hold the poll off"
        );
    }

    /// A result for a repository evicted mid-fetch inserts nothing.
    ///
    /// `merge_tier0` inserts where there was no row — deliberately, so `refresh_repo` can retry a
    /// §8.1 total failure. After `remove_root` or a completed scan's eviction that would
    /// **resurrect** a row under no configured root, which `retain_scanned` can never evict again
    /// because it is scoped to the roots it walked, and which then survives into `cache.json` and
    /// comes back next launch. Silent and permanent, so it is guarded and tested.
    #[test]
    fn a_result_for_an_evicted_repository_resurrects_no_row() {
        let (_dir, repo) = tree();
        let state = Arc::new(AppState::default());
        // Never recorded as discovered, which is the state eviction leaves behind.
        let results = vec![FetchOutcome {
            path: repo.path.clone(),
            status: FetchStatus::Ok,
            detail: None,
            elapsed_ms: 1,
        }];

        let rows = settle(&state, &results, Duration::ZERO);

        assert!(rows.is_empty(), "nothing to re-read");
        assert!(
            state.row(&repo.path).is_none(),
            "and above all, nothing inserted"
        );
    }

    /// A guard dropped without finishing still emits one terminal event and sweeps, which is what
    /// stops a panic leaving a spinner and a dead row behind.
    #[test]
    fn an_unwound_pass_still_ends_and_still_releases() {
        let state = Arc::new(AppState::default());
        let (channel, seen) = collecting_channel();
        let path = PathBuf::from("C:/work/alpha");
        let (id, _cancel) = state.begin_fetch();
        state.begin_fetch_group(std::slice::from_ref(&path), Duration::from_secs(600));

        drop(FetchGuard::new(Arc::clone(&state), id, channel, vec![path]));

        let events = seen.lock().expect("not poisoned").clone();
        assert_eq!(kinds(&events), vec!["finished"]);
        assert!(matches!(
            events.first(),
            Some(FetchEvent::Finished { summary }) if summary.cancelled
        ));
        assert!(!state.fetch_in_flight(), "the sweep ran");
    }
}
