//! Starting and stopping scans.

use std::{path::PathBuf, sync::Arc};

use anyhow::anyhow;
use repo_scan::{ScanEvent, ScanId, ScanOpts};
use tauri::{AppHandle, State, ipc::Channel};

use crate::{error::CommandResult, state::AppState};

/// Start a scan and return its id immediately.
///
/// Every path must already be a configured root. `add_root` is the single place a new path enters
/// the app and the only place it is canonicalised and checked, so validating against that list
/// here is what keeps an arbitrary string from the webview out of the walk.
///
/// Starting a scan cancels every scan already running: a root change or a rescan supersedes what
/// came before, and their in-flight batches carry a stale `ScanId` that the frontend drops anyway.
///
/// `async` so the handler never runs inline on the event-loop thread. It does not await the
/// pipeline — `spawn_blocking` is fired and the id returned, because the id is what the caller
/// needs in order to cancel.
///
/// Not awaiting the pipeline is also why the row cache is written from a closure handed *to* it
/// rather than after an await here: by the time this command returns, the scan has not started.
#[tauri::command]
pub async fn scan_roots(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    roots: Vec<PathBuf>,
    opts: ScanOpts,
    on_event: Channel<ScanEvent>,
) -> CommandResult<ScanId> {
    if roots.is_empty() {
        return Err(anyhow!("no roots to scan").into());
    }
    for root in &roots {
        if !state.has_root(root) {
            return Err(anyhow!("`{}` is not a configured root", root.display()).into());
        }
    }

    state.cancel_all();
    let (id, cancel) = state.begin_scan();

    // `on_event` is moved into the closure before this command returns, so a live clone outlasts
    // the invocation and the channel's `{ end: true }` drop hook does not fire early. The
    // `JoinHandle` is deliberately dropped: nothing awaits the pipeline, and the task runs on.
    let handle = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        crate::pipeline::run_scan(handle, id, roots, opts, cancel, on_event, |state| {
            // After the terminal event, so nothing a user is waiting for is behind this write.
            crate::persist::save_cache(&app, state);
        });
    });

    Ok(id)
}

/// Flip a scan's cancellation flag.
///
/// Synchronous on purpose: one lock and one atomic store. Putting the one command whose entire job
/// is to be immediate behind the async runtime's queue would be perverse.
///
/// Cancelling an id that has already finished is a no-op rather than an error — the frontend
/// cannot know the scan ended between rendering the button and the click.
#[tauri::command]
pub fn cancel_scan(state: State<'_, Arc<AppState>>, id: ScanId) {
    state.cancel_scan(id);
}
