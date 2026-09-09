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
//! app's own commands. The frontend installs none of the `@tauri-apps/plugin-*` packages, so
//! `capabilities/default.json` grants `core:default` and nothing else — the ACL is enforced only
//! on webview-initiated calls, so a Rust-side plugin call never consults it.
//!
//! The corollary is that commands validate their own inputs. Plugin scopes are not a guard here;
//! commands taking a path accept only a configured root or a key already present in the canonical
//! repo map.

mod commands;
mod error;
mod pipeline;
mod state;
mod stream;

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

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // store and window-state expose no `init()` — only a builder.
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(Arc::new(AppState::default()))
        // Closing the window flips every live scan's flag, so the blocking threads unwind while
        // the runtime is still up. `CloseRequested` and not `Destroyed` for that reason. This is
        // about not burning cores on results nobody will see: a send into a dead webview returns
        // `Ok(())` either way, so it is not an error path.
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                use tauri::Manager as _;
                window.state::<Arc<AppState>>().cancel_all();
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
