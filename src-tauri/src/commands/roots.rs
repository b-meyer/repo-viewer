//! Choosing and managing the folders to scan.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, anyhow};
use repo_scan::RepoEvent;
use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::{error::CommandResult, persist, state::AppState};

/// Show the native folder picker. `None` when the user cancels.
///
/// Uses the callback form with a `oneshot`, not `blocking_pick_folder`. The blocking variant is a
/// rendezvous `sync_channel(0)` with `rx.recv().unwrap()`: it **panics** rather than returning
/// `None` if the callback never fires — which is what happens when the dialog's own
/// `run_on_main_thread` fails during teardown — and it parks a blocking-pool thread for as long as
/// the user stares at the dialog. A dropped sender here is an ordinary command failure instead.
///
/// This only picks. The path round-trips through the webview and comes back to [`add_root`], which
/// re-validates it: a path from the webview is a path from the webview regardless of where the
/// webview says it got it.
#[tauri::command]
pub async fn pick_root(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
) -> CommandResult<Option<PathBuf>> {
    // Read and release before the await. An `std::sync` guard held across an await point makes the
    // future `!Send`, and a Tauri async command's future must be `Send`.
    let seed = state.roots().last().cloned();

    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut builder = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Choose a folder to scan");
    if let Some(seed) = seed {
        builder = builder.set_directory(seed);
    }
    builder.pick_folder(move |picked| {
        let _ = tx.send(picked);
    });

    let Some(picked) = rx
        .await
        .context("the folder dialog closed without answering")?
    else {
        return Ok(None);
    };

    // `simplified()` drops the Windows verbatim prefix, matching the `dunce` policy discovery
    // applies. `add_root` canonicalises again, so this only has to be presentable.
    let path = picked
        .simplified()
        .into_path()
        .context("the folder dialog returned a URL rather than a path")?;
    Ok(Some(path))
}

/// Canonicalise, validate, and add a root. Returns the new list.
///
/// `async` with the filesystem work on `spawn_blocking`: canonicalising is a syscall, and on a
/// stale drive letter or a disconnected share it can block for seconds. Inline on the event loop
/// that would freeze the window.
///
/// This is the single place a new path enters the app, so it is also where the root list is
/// persisted. The write is best-effort — the root is already in memory and this command has
/// succeeded, so a failed write is a log line rather than a refusal.
#[tauri::command]
pub async fn add_root(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: PathBuf,
) -> CommandResult<Vec<PathBuf>> {
    let resolved = tauri::async_runtime::spawn_blocking(move || {
        if !path.is_dir() {
            return Err(anyhow!("`{}` is not a folder", path.display()));
        }
        // Through `dunce`, so the key matches the one discovery produces for the same folder.
        // Falling back to the given path keeps a root that cannot be canonicalised usable rather
        // than rejecting it outright.
        Ok(repo_scan::canonical(&path))
    })
    .await
    .context("the folder check did not finish")??;

    let roots = state.add_root(resolved);
    persist::save_roots(&app, &roots);
    Ok(roots)
}

/// Remove a root and evict every row beneath it. Returns the new list.
///
/// Canonicalises to match the stored key, falling back to an exact match when that fails — a root
/// on a drive that has since been unplugged must still be removable, which is the one case where
/// refusing to normalise is the correct behaviour.
#[tauri::command]
pub async fn remove_root(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: PathBuf,
) -> CommandResult<Vec<PathBuf>> {
    let resolved = tauri::async_runtime::spawn_blocking(move || repo_scan::canonical(&path))
        .await
        .context("the folder check did not finish")?;

    let (roots, evicted) = state.remove_root(&resolved);
    persist::save_roots(&app, &roots);
    if !evicted.is_empty() {
        // A row change untied to any scan, which is exactly what the session channel is for.
        state.push(RepoEvent::Removed { paths: evicted });
    }
    // The rows are gone from both maps, so the watches over them are now watches over nothing.
    // Registration failures are impossible on a removal — nothing is being added — so the return
    // value has nothing to report and is deliberately dropped.
    let _ = state.sync_watches();
    Ok(roots)
}

/// The configured roots. Synchronous: one lock.
#[tauri::command]
pub fn list_roots(state: State<'_, Arc<AppState>>) -> Vec<PathBuf> {
    state.roots()
}
