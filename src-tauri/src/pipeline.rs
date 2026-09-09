//! The scan driver: discovery streaming into Tier 0, both reporting over one channel.
//!
//! This is where the tiering claim is cashed. Discovery streams repositories into a batcher; each
//! batch is emitted as [`ScanEvent::ReposFound`] — so a row paints before any refs are read — and
//! only then handed to Tier 0, whose merged rows follow as [`ScanEvent::ReposUpdated`]. The walk
//! keeps descending while Tier 0 reads, which is the overlap the engine's half-core walker thread
//! count was chosen for.
//!
//! Nothing here is `async`. The whole pipeline runs on one `spawn_blocking` thread, and the walker
//! and rayon threads below it are plain OS threads the tokio runtime knows nothing about — which
//! is what keeps a filesystem read from ever parking a runtime worker.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};

use repo_scan::{
    DiscoveredRepo, ScanError, ScanEvent, ScanId, ScanOpts, ScanTotals, discover_roots_with,
    read_tier0_all_with, read_tier1_all_with,
};
use tauri::ipc::Channel;

use crate::{
    state::AppState,
    stream::{BATCH_MAX, BATCH_WINDOW, next_batch},
};

/// Run one scan to completion on the calling thread.
///
/// Returns when the walk and every Tier 0 batch are done, or when `cancel` is flipped. Either way
/// the scan is deregistered and exactly one terminal event — [`ScanEvent::Finished`] or
/// [`ScanEvent::Cancelled`] — reaches the frontend.
pub fn run_scan(
    state: Arc<AppState>,
    id: ScanId,
    roots: Vec<PathBuf>,
    opts: ScanOpts,
    cancel: Arc<AtomicBool>,
    events: Channel<ScanEvent>,
) {
    let started = Instant::now();
    let mut guard = ScanGuard::new(Arc::clone(&state), id, events.clone());

    let (found_tx, found_rx) = mpsc::channel::<DiscoveredRepo>();

    // The walk runs on its own thread so this one can drain and read at the same time. `found_tx`
    // is moved into the closure, which `discover_roots_with` drops on return — that disconnect is
    // what ends the drain loop below, so there is no separate "walk is done" signal to get wrong.
    //
    // The walk's own duration comes back through this cell rather than being timed out here: the
    // thread is joined after the drain loop, so measuring around the join would include every
    // tier's time as well.
    let discovery_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let walker = std::thread::spawn({
        let (cancel, events, opts) = (Arc::clone(&cancel), events.clone(), opts.clone());
        let discovery_ms = Arc::clone(&discovery_ms);
        move || {
            let summary = discover_roots_with(&roots, &opts, &cancel, move |repo| {
                let _ = found_tx.send(repo);
            });
            discovery_ms.store(summary.elapsed_ms, Ordering::Relaxed);
            emit(
                &events,
                ScanEvent::DiscoveryFinished {
                    scan_id: id,
                    summary,
                },
            );
        }
    });

    let mut errors: Vec<ScanError> = Vec::new();
    let mut tier0_ms = 0_u64;
    let mut tier1_ms = 0_u64;

    while let Some(batch) = next_batch(&found_rx, BATCH_MAX, BATCH_WINDOW) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        guard.found += batch.len() as u32;
        // Recorded before the batch is emitted, so a `full_status` or `refresh_repo` arriving the
        // instant a row paints can already resolve its Git directory.
        state.record_found(&batch);
        // Cloned rather than borrowed: `Channel::send` takes ownership to serialize, and Tier 0
        // needs the batch afterwards. Five `PathBuf`s times 25 is nothing beside 25 x ~3 ms of
        // refs I/O — and the alternative, reading first and emitting once, is precisely the
        // tiering this phase exists to deliver, undone.
        emit(
            &events,
            ScanEvent::ReposFound {
                scan_id: id,
                repos: batch.clone(),
            },
        );

        let rows = Mutex::new(Vec::with_capacity(batch.len()));
        let summary = read_tier0_all_with(&batch, &cancel, |status| {
            lock(&rows).push(status);
        });
        tier0_ms = tier0_ms.saturating_add(summary.elapsed_ms);

        // The total-failure grade, delivered now rather than only at the end. These repositories
        // will never produce a row, so without this their rows would read "counting…" — a claim
        // that work is in progress — for the rest of the scan, which on a large tree is most of a
        // minute.
        if !summary.errors.is_empty() {
            emit(
                &events,
                ScanEvent::RepoErrors {
                    scan_id: id,
                    errors: summary.errors.clone(),
                },
            );
        }
        errors.extend(summary.errors);

        let read = lock_into(rows);
        // Rows read before a mid-batch cancellation are kept and merged. The engine documents that
        // a cancelled pass "does not discard them", and throwing away work already paid for only
        // to render "counting…" for data Rust holds would be the honesty rule inverted.
        let merged = state.merge_tier0_batch(read);
        guard.read += merged.len() as u32;
        if !merged.is_empty() {
            emit(
                &events,
                ScanEvent::ReposUpdated {
                    scan_id: id,
                    repos: merged,
                },
            );
        }
        emit(
            &events,
            ScanEvent::Progress {
                scan_id: id,
                found: guard.found,
                read: guard.read,
            },
        );

        // Tier 1 follows on the same batch, so a row's refs are on screen before its worktree is
        // touched. Emitted as its own `ReposUpdated` for that reason: folding it into the Tier 0
        // send would hold the cheap answer back for the expensive one.
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let dirty = Mutex::new(Vec::with_capacity(batch.len()));
        let tier1 = read_tier1_all_with(&batch, &cancel, |status| {
            lock(&dirty).push(status);
        });
        tier1_ms = tier1_ms.saturating_add(tier1.elapsed_ms);
        let merged = state.merge_tier1_batch(lock_into(dirty));
        if !merged.is_empty() {
            emit(
                &events,
                ScanEvent::ReposUpdated {
                    scan_id: id,
                    repos: merged,
                },
            );
        }

        // A Tier 1 total failure still has a Tier 0 row, so the cause goes onto that row instead
        // of into the terminal summary, which lists repositories that produced no row at all.
        if !tier1.errors.is_empty() {
            let flagged = state.record_errors(&tier1.errors);
            if !flagged.is_empty() {
                emit(
                    &events,
                    ScanEvent::ReposUpdated {
                        scan_id: id,
                        repos: flagged,
                    },
                );
            }
        }
    }

    if walker.join().is_err() {
        tracing::error!(scan = %id, "the discovery walk panicked");
    }

    if cancel.load(Ordering::Relaxed) {
        return; // The guard emits `Cancelled` with the counts it holds.
    }

    guard.finish(ScanTotals {
        repos_read: guard.read,
        errors,
        // Wall clock across the whole pipeline: the walk and every tier. This is the number a user
        // experiences, and it is **not** the sum of the three fields below — most of a scan is
        // spent waiting for the walk to hand over the next batch, which belongs to none of them.
        elapsed_ms: elapsed_ms(started),
        discovery_ms: discovery_ms.load(Ordering::Relaxed),
        // Sums of the per-batch passes. Attribution rather than duration: the batches run one
        // after another, so the spans do not overlap, and this is what says which tier the work
        // went into. Tier 1 is where roughly 24x of it goes cold.
        tier0_ms,
        tier1_ms,
    });
}

/// Lock through a poison rather than cascading one worker's panic into the pipeline.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Take a mutex's contents, recovering from a poison.
fn lock_into<T>(mutex: Mutex<T>) -> T {
    mutex.into_inner().unwrap_or_else(PoisonError::into_inner)
}

/// Sends `event`, logging rather than propagating a failure.
///
/// `send`'s `Err` is not a liveness signal: it returns `Ok(())` for a closed webview and only
/// fails once the app is shutting down. Letting it gate the pipeline would stop a scan for the one
/// condition where stopping changes nothing.
fn emit(channel: &Channel<ScanEvent>, event: ScanEvent) {
    if let Err(error) = channel.send(event) {
        tracing::warn!(%error, "could not send a scan event");
    }
}

/// Deregisters a scan and guarantees exactly one terminal event.
///
/// The terminal event is emitted from `Drop`, so it survives an unwind. Without that, a panic
/// anywhere in the pipeline would leave a registry entry behind and a spinner running forever —
/// and nobody would see it, because nothing awaits the `spawn_blocking` handle.
struct ScanGuard {
    state: Arc<AppState>,
    id: ScanId,
    events: Channel<ScanEvent>,
    /// Repositories reported found so far.
    found: u32,
    /// Rows merged so far.
    read: u32,
    /// Set once a terminal event has been emitted, so `Drop` does not send a second.
    finished: bool,
}

impl ScanGuard {
    /// Start guarding `id`.
    fn new(state: Arc<AppState>, id: ScanId, events: Channel<ScanEvent>) -> Self {
        Self {
            state,
            id,
            events,
            found: 0,
            read: 0,
            finished: false,
        }
    }

    /// Emit [`ScanEvent::Finished`]. The scan completed.
    fn finish(&mut self, summary: ScanTotals) {
        self.finished = true;
        emit(
            &self.events,
            ScanEvent::Finished {
                scan_id: self.id,
                summary,
            },
        );
    }
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        if !self.finished {
            emit(
                &self.events,
                ScanEvent::Cancelled {
                    scan_id: self.id,
                    found: self.found,
                    read: self.read,
                },
            );
        }
        self.state.finish_scan(self.id);
    }
}

/// Milliseconds since `started`, saturating.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Collects events from a **real** `Channel<ScanEvent>`.
    ///
    /// `Channel::new` needs no `AppHandle` and no `Webview`, so the pipeline is exercised against
    /// the genuine article rather than a mock — and the assertions run over what actually crosses
    /// the wire, since the handler receives the serialized body and parses it back. That is also
    /// why there is no `EventSink` trait here: the real channel removed the reason for one.
    fn collecting_channel() -> (Channel<ScanEvent>, Arc<Mutex<Vec<ScanEvent>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let channel = Channel::new(move |body| {
            let json = body.deserialize::<ScanEvent>().expect("a ScanEvent body");
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(json);
            Ok(())
        });
        (channel, seen)
    }

    /// This repository's own root — a tree guaranteed to hold at least one repository.
    ///
    /// `repo-scan`'s fixtures live in its `tests/support/` and are not reachable from here, so
    /// scanning ourselves avoids either exposing them behind a new feature or duplicating them.
    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri has a parent")
            .to_path_buf()
    }

    fn shallow_opts() -> ScanOpts {
        ScanOpts {
            max_depth: Some(1),
            ..ScanOpts::default()
        }
    }

    #[test]
    fn a_scan_streams_rows_and_ends_with_finished() {
        let root = workspace_root();
        if !root.join(".git").exists() {
            return; // Not a checkout — nothing to scan.
        }

        let state = Arc::new(AppState::default());
        let (id, cancel) = state.begin_scan();
        let (channel, seen) = collecting_channel();

        run_scan(
            Arc::clone(&state),
            id,
            vec![root],
            shallow_opts(),
            cancel,
            channel,
        );

        let events = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert!(
            events.iter().all(|event| scan_id_of(event) == id),
            "every event carries the id of the scan that produced it"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ScanEvent::DiscoveryFinished { .. }))
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ScanEvent::ReposFound { .. }))
        );

        // Distinct paths, not the sum of batch lengths: each tier emits its own `ReposUpdated` for
        // the same row, so summing would count a row once per tier that touched it.
        let updated: std::collections::HashSet<&std::path::Path> = events
            .iter()
            .filter_map(|event| match event {
                ScanEvent::ReposUpdated { repos, .. } => Some(repos),
                _ => None,
            })
            .flatten()
            .map(|row| row.path.as_path())
            .collect();
        assert!(!updated.is_empty(), "at least one row was read");

        // Tier 1 runs on the same batch, so a completed scan has answered the worktree question
        // for every row it read rather than leaving it for later.
        assert!(
            events.iter().any(|event| matches!(
                event,
                ScanEvent::ReposUpdated { repos, .. } if repos.iter().any(|row| row.dirty.is_some())
            )),
            "a finished scan reports the dirty flag, not just the refs"
        );

        let Some(ScanEvent::Finished { summary, .. }) = events.last() else {
            panic!("the terminal event is Finished, got {:?}", events.last());
        };
        assert_eq!(summary.repos_read as usize, updated.len());
        // Every row that went over the wire is in the canonical map, which is what makes the
        // mirror on the other side a mirror rather than a second source of truth.
        assert_eq!(state.snapshot().len(), updated.len());
    }

    #[test]
    fn a_cancelled_scan_ends_with_cancelled() {
        let state = Arc::new(AppState::default());
        let (id, cancel) = state.begin_scan();
        // Pre-set, so the outcome does not depend on winning a race with the walk.
        cancel.store(true, Ordering::Relaxed);
        let (channel, seen) = collecting_channel();

        run_scan(
            Arc::clone(&state),
            id,
            vec![workspace_root()],
            shallow_opts(),
            cancel,
            channel,
        );

        let events = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert!(matches!(events.last(), Some(ScanEvent::Cancelled { .. })));

        // The guard runs on every path, so a cancelled scan leaves no registry entry behind.
        state.cancel_scan(id);
    }

    /// The id every variant carries, for the "no stale rows" assertion.
    fn scan_id_of(event: &ScanEvent) -> ScanId {
        match event {
            ScanEvent::ReposFound { scan_id, .. }
            | ScanEvent::ReposUpdated { scan_id, .. }
            | ScanEvent::Progress { scan_id, .. }
            | ScanEvent::DiscoveryFinished { scan_id, .. }
            | ScanEvent::RepoErrors { scan_id, .. }
            | ScanEvent::Finished { scan_id, .. }
            | ScanEvent::Cancelled { scan_id, .. } => *scan_id,
        }
    }
}
