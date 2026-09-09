//! The session channel.

use std::sync::Arc;

use repo_scan::{RepoEvent, RepoStatus};
use tauri::{State, ipc::Channel};

use crate::state::AppState;

/// Open the session channel for this app session and return the rows Rust already holds.
///
/// The channel is stored by value rather than used and dropped. A channel taken as a command
/// argument installs an `on_drop` hook that ends it on the JS side, so letting the argument fall
/// out of scope here would close it the instant it was opened.
///
/// The snapshot is what makes a webview reload — which happens constantly under Vite HMR — repaint
/// from the canonical map instead of forcing a rescan. The frontend mirrors these rows exactly as
/// it mirrors a scan batch.
///
/// Synchronous: one lock and a move. A sync command runs inline on the event-loop thread, which is
/// correct only for work this small.
#[tauri::command]
pub fn subscribe(state: State<'_, Arc<AppState>>, on_event: Channel<RepoEvent>) -> Vec<RepoStatus> {
    let rows = state.snapshot();
    // Logged because a repeated `subscribe` is the signature of the webview reloading, and under
    // `tauri dev` that is otherwise invisible: Vite reports a client reload over `console.log`,
    // which is not forwarded to the terminal, while the stale channel it leaves behind shows up
    // only as a `Couldn't find callback id` warning with no cause attached.
    tracing::debug!(
        channel = on_event.id(),
        rows = rows.len(),
        "session channel opened"
    );
    state.set_session(on_event);
    rows
}
