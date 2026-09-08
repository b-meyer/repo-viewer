//! Tauri glue. Commands, state, and channel adaptation — nothing else.
//!
//! All discovery, Git reads, watching, and fetch live in the `repo-scan` crate. If something in
//! here starts doing engine work, it belongs there instead.
//!
//! # Plugins are called from Rust
//!
//! `dialog`, `opener`, `store`, and `window-state` are registered below and reached through this
//! app's own commands. The frontend installs none of the `@tauri-apps/plugin-*` packages, so
//! `capabilities/default.json` grants `core:default` and nothing else — the ACL is enforced only
//! on webview-initiated calls, so a Rust-side plugin call never consults it.
//!
//! The corollary is that commands validate their own inputs. Plugin scopes are not a guard here;
//! commands taking a path accept only a key already present in the canonical repo map.

/// Registers plugins and commands, then runs the app.
///
/// # Panics
///
/// Panics if the Tauri context fails to build, which means the app cannot start at all.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // store and window-state expose no `init()` — only a builder.
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Liveness check for the IPC path.
///
/// The frontend calls this on load, so a broken bridge shows up as a visible failure on the page
/// rather than as silence.
#[tauri::command]
fn ping() -> &'static str {
    "pong"
}
