//! Tauri glue. Commands, state, and channel adaptation — nothing else.
//!
//! All discovery, Git reads, watching, and fetch live in the `repo-scan` crate. If something in
//! here starts doing engine work, it belongs there instead. What this crate owns is the canonical
//! row state ([`state`]), the batching that turns a stream of rows into channel sends ([`stream`]),
//! the scan driver that connects the two ([`pipeline`]), and the commands the webview calls.
//!
//! # Plugins are called from Rust
//!
//! `dialog`, `opener`, `store`, and `window-state` are registered below and reached through this
//! app's own commands — `store` only ever through [`persist`], which is the one module that names
//! it. The frontend installs none of the `@tauri-apps/plugin-*` packages, so
//! `capabilities/default.json` grants `core:default` and nothing else — the ACL is enforced only
//! on webview-initiated calls, so a Rust-side plugin call never consults it.
//!
//! The corollary is that commands validate their own inputs. Plugin scopes are not a guard here;
//! commands taking a path accept only a configured root or a key already present in the canonical
//! repo map.

mod commands;
mod error;
mod fetch;
mod live;
mod persist;
mod pipeline;
mod state;
mod stream;
mod webview2;

use std::sync::Arc;

use state::AppState;

/// Registers plugins and commands, then runs the app.
///
/// # Panics
///
/// Panics if the Tauri context fails to build, which means the app cannot start at all.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();
    // Before the builder, not in `setup`: windows declared in `tauri.conf.json` are created during
    // `build()`, so by the time `setup` runs a missing webview has already produced the blank exit
    // this replaces with an explanation.
    webview2::ensure();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // store and window-state expose no `init()` — only a builder.
        .plugin(tauri_plugin_store::Builder::new().build())
        // Registration is the whole of window-state: its default `StateFlags` is `all()`, it
        // restores on window-ready, and it saves on `RunEvent::Exit`. There is no restore call to
        // make, and adding one would run it twice.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(Arc::new(AppState::default()))
        // Runs once, before any command can be invoked, which is what lets [`AppState::restore`]
        // insert rather than merge. A cached row paints as soon as `subscribe` returns it, so
        // showing one needs no new command and no change on the other side of the boundary.
        .setup(|app| {
            use tauri::Manager as _;

            let state = app.state::<Arc<AppState>>();
            persist::load(app.handle(), state.inner());

            // Probed once so the fetch controls can be disabled with a reason rather than failing
            // at click time (§10.2). It is only that affordance: `fetch_repos` resolves `git`
            // again per invocation, because a `PATH` can change while the app runs.
            let git = repo_scan::probe_git();
            match &git {
                Some(info) => {
                    tracing::info!(version = %info.version, path = %info.path.display(), "git")
                }
                None => tracing::info!("no usable `git` on PATH; fetching is unavailable"),
            }
            state.set_git(git);

            // After the cache is loaded, so the poll has the restored `found` map to work from if it
            // fires before the launch scan finishes. Nothing is watched yet: the watch set follows
            // from what discovery finds, and the first `sync_watches` is the launch scan's.
            live::start(state.inner(), &persist::watch(app.handle()));
            Ok(())
        })
        // Closing the window flips every live scan's flag and stops the watcher and the poll, so
        // the blocking threads unwind while the runtime is still up. `CloseRequested` and not
        // `Destroyed` for that reason. This is about not burning cores on results nobody will see:
        // a send into a dead webview returns `Ok(())` either way, so it is not an error path.
        //
        // Focus is the other half of §7.4's safety net. It shares the poll's thread rather than
        // having one of its own — the tick wakes it early — which is what makes "the poll and the
        // focus handler take the same path" true of the code and not just of the design.
        .on_window_event(|window, event| {
            use tauri::Manager as _;

            match event {
                tauri::WindowEvent::CloseRequested { .. } => {
                    let state = window.state::<Arc<AppState>>();
                    state.cancel_all();
                    // Not the same as cancelling a scan, and not optional: a fetch owns child
                    // `git` processes. Without this they are orphaned when the app exits, and
                    // keep writing into the user's repositories — holding directory handles open
                    // on Windows — for as long as their transport takes to finish.
                    state.cancel_fetches();
                    state.stop_live();
                }
                tauri::WindowEvent::Focused(true) => {
                    window.state::<Arc<AppState>>().tick(live::Tick::Refresh);
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            commands::subscribe,
            commands::scan_roots,
            commands::cancel_scan,
            commands::full_status,
            commands::refresh_repo,
            commands::pick_root,
            commands::add_root,
            commands::remove_root,
            commands::list_roots,
            commands::open_in,
            commands::fetch_repos,
            commands::cancel_fetch,
            commands::git_info,
            commands::ui_settings,
            commands::save_ui_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the tauri application")
        // `build` then `run`, rather than `Builder::run`, for the one event the builder cannot
        // hand over: `Exit`. A scan writes the cache when it ends, but a `refresh_repo` or an
        // expanded drawer changes rows afterwards, and this is what keeps those.
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                use tauri::Manager as _;

                let state = app.state::<Arc<AppState>>();
                persist::save_cache(app, state.inner());
            }
        });
}

/// Install the log subscriber.
///
/// Without this every `tracing::` call in the app is discarded, which is worse than having none:
/// the code reads as though it reports a failure and reports nothing. The three that matter are a
/// channel send that failed, a session push that failed, and a panicking discovery walk — the last
/// of which would otherwise be completely silent, because nothing awaits the `spawn_blocking`
/// handle that would carry the panic.
///
/// Filtered by level rather than by target, which needs no `env-filter` feature and so no regex
/// engine in the dependency tree. `RUST_LOG`-style per-target filtering is the upgrade if it is
/// ever wanted, and costs that feature plus its transitive dependencies.
///
/// `try_init` rather than `init` because a second call must not panic the app over logging.
fn init_tracing() {
    let level = if cfg!(debug_assertions) {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(true)
        .try_init();
}

/// Liveness check for the IPC path.
///
/// The frontend calls this on load, so a broken bridge shows up as a visible failure on the page
/// rather than as silence. It also separates "IPC is dead" from "`subscribe` specifically failed",
/// which is why it survives now that there is a real command surface beside it.
#[tauri::command]
fn ping() -> &'static str {
    "pong"
}
