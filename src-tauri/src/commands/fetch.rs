//! Fetching, and finding the `git` that does it.
//!
//! The only commands in this app that cause a write, and the only ones that touch a network.
//!
//! # Two validation domains again, and this one checks the superset
//!
//! [`fetch_repos`] validates against the **discovered** map rather than the rows, on
//! `refresh_repo`'s precedent. A repository whose HEAD could not be read has no row — and its
//! ahead/behind is therefore unreadable, which makes it arguably the one most worth fetching.
//! Refusing it would be the wrong reading of §6.1.
//!
//! # `git` is resolved twice, on purpose
//!
//! `setup` probes once and the result decides whether the buttons are enabled. That is an
//! affordance, not the truth: a `PATH` can change and `git` can be uninstalled while the app runs
//! — exactly the argument §10.2 already makes for the editor and terminal, and which applies here
//! no less. So this command resolves `git` again, once per invocation, and refuses outright if it
//! has gone. The engine reports [`repo_scan::FetchStatus::GitMissing`] per repository on top of
//! that, for the case where it disappears mid-pass.

use std::{path::PathBuf, sync::Arc};

use anyhow::anyhow;
use repo_scan::{DiscoveredRepo, FetchEvent, FetchId, FetchOpts, GitInfo};
use tauri::{AppHandle, State, ipc::Channel};

use crate::{error::CommandResult, live, persist, state::AppState};

/// Start a fetch over `paths` and return its id immediately.
///
/// `async` so the handler never runs inline on the event-loop thread, and it does **not** await
/// the pass: `spawn_blocking` is fired and the id returned, because the id is what the caller
/// needs in order to cancel. Progress arrives on `on_event`; the refreshed rows arrive on the
/// session channel.
///
/// Starting one does not cancel another. A scan supersedes the scan before it — same question,
/// same tree — where two fetches are two sets of repositories a user asked for.
#[tauri::command]
pub async fn fetch_repos(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    paths: Vec<PathBuf>,
    on_event: Channel<FetchEvent>,
) -> CommandResult<FetchId> {
    if paths.is_empty() {
        return Err(anyhow!("no repositories to fetch").into());
    }

    // Rust's own copies of the map keys, never the strings the webview sent. The lookup *is* the
    // validation, so its results are what the pass runs on.
    let mut repos: Vec<DiscoveredRepo> = Vec::with_capacity(paths.len());
    for path in &paths {
        let found = state
            .discovered(path)
            .ok_or_else(|| anyhow!("`{}` is not a known repository", path.display()))?;
        repos.push(found);
    }

    let git = repo_scan::probe_git()
        .ok_or_else(|| anyhow!("`git` could not be found or run, so fetching is not available"))?;

    let settings = persist::fetch(&app);
    let opts = FetchOpts {
        program: git.path,
        timeout: settings.timeout(),
        concurrency: settings.concurrency(),
        // A single repository is a deliberate click and is never guarded; a batch is where §8.2's
        // "fetching 300 repos unprompted is hostile" applies.
        min_interval: (repos.len() > 1).then(|| settings.min_interval()).flatten(),
        prune: settings.prune,
        all_remotes: settings.all_remotes,
    };
    let tail = live::suppress_tail(&persist::watch(&app));

    let (id, cancel) = state.begin_fetch();

    // `on_event` is moved into the closure before this command returns, so a live clone outlasts
    // the invocation and the channel's `{ end: true }` drop hook does not fire early. The
    // `JoinHandle` is deliberately dropped: nothing awaits the pass, and it runs on.
    let handle = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        crate::fetch::run_fetch(handle, id, repos, opts, tail, cancel, on_event);
    });

    Ok(id)
}

/// Flip one fetch pass's cancellation flag.
///
/// Synchronous, for the reason `cancel_scan` is: one lock and one atomic store, and putting the
/// command whose entire job is to be immediate behind the async runtime's queue would be perverse.
///
/// Cancelling an id that has already finished is a no-op rather than an error.
#[tauri::command]
pub fn cancel_fetch(state: State<'_, Arc<AppState>>, id: FetchId) {
    state.cancel_fetch(id);
}

/// The `git` found at startup, or `None` when there is none.
///
/// A command of its own rather than a field on `subscribe`'s reply: that command returns
/// `Vec<RepoStatus>` and is what the whole app hangs off, so widening it to a struct would touch
/// every one of its tests to carry one optional value. One small command per concern is the shape
/// `list_roots` and `ui_settings` already have.
#[tauri::command]
pub fn git_info(state: State<'_, Arc<AppState>>) -> Option<GitInfo> {
    state.git()
}
