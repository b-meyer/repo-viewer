//! The view state, persisted.
//!
//! Chips, sort key and grouping are facts about this window, not about a repository — so unlike the
//! rows, Rust does not own them, it keeps them. The value crosses as opaque JSON and
//! `src/scripts/settings.ts` owns its shape; `persist::ui` carries the reasoning for that, and the
//! cost, which is that the frontend must parse what comes back rather than trust it.
//!
//! Both commands are synchronous, and that is safe for a specific reason: `setup` has already opened
//! the settings file, so `persist` hands back an in-memory store and neither of these touches the
//! disk. The write that eventually reaches it is the plugin's own debounced auto-save, on its own
//! task.

use anyhow::Context;
use serde_json::Value as JsonValue;
use tauri::AppHandle;

use crate::{error::CommandResult, persist};

/// The persisted view state, or `null` when nothing has been saved yet.
///
/// `null` rather than an invented default: what the defaults are is a question about the table, and
/// the table is what answers it. Rust supplying one would be a second opinion on a shape it does not
/// otherwise read.
#[tauri::command]
pub fn ui_settings(app: AppHandle) -> CommandResult<JsonValue> {
    Ok(persist::ui(&app).context("could not read the saved view")?)
}

/// Persist the view state.
///
/// Fallible, unlike the root list's persistence: there the value is already in memory and the user's
/// action has succeeded regardless, whereas here the write *is* the action, so a failure is the
/// caller's to know about.
#[tauri::command]
pub fn save_ui_settings(app: AppHandle, ui: JsonValue) -> CommandResult<()> {
    Ok(persist::save_ui(&app, &ui).context("could not save the view")?)
}
