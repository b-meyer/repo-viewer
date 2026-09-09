//! Settings and the row cache, on `tauri-plugin-store`.
//!
//! **This is the only file that touches `tauri_plugin_store`.** Everything else asks for a value or
//! hands one over; where it is kept, in what shape, and how tolerantly it is read are all decided
//! here. That is the same containment `ipc.ts` gives `@tauri-apps/api` on the other side of the
//! boundary.
//!
//! # Two files, because they have two lifetimes
//!
//! `settings.json` is what the user meant — the roots, the commands `open_in` runs, and the view
//! state the table renders from. It is tiny and written when the user does something, so the
//! plugin's own auto-save is exactly right for it.
//!
//! `cache.json` is what the app last saw. It is the whole row map, hundreds of kilobytes on a real
//! tree, and it is written once at the end of a scan. **Its store must disable auto-save**, because
//! `StoreBuilder` defaults to `auto_save: Some(100 ms)` and every `set` restarts that debounce —
//! left on, one scan would serialise the entire map after every write.
//!
//! # The cache is a claim about the past
//!
//! A cached row paints at launch so the window is not empty, and it carries its original
//! `scanned_at_ms`, which the table already renders as an age. That age is what makes it honest, so
//! nothing here refreshes it.
//!
//! Tier 2 is the exception and is **dropped on load**: the drawer shows its counts with no age
//! beside them, so a cached count would read as freshly measured. Dropped, the first expand reads
//! them again and says `counting…` while it does — which is true.
//!
//! # Nothing here can stop the app from starting
//!
//! A missing file, a corrupt one, a shape from another version, a hand-edit that does not parse:
//! each is logged and skipped, and the app then behaves as it did before there was a cache, which
//! is a state it already handles. `StoreBuilder::build` takes the same view — it ignores a failed
//! load and hands back an empty store.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use repo_scan::{DiscoveredRepo, RepoStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::{Store, StoreExt};

use crate::state::AppState;

/// What the user meant. Auto-saved by the plugin.
const SETTINGS_FILE: &str = "settings.json";

/// What the app last saw. Written explicitly.
const CACHE_FILE: &str = "cache.json";

/// The configured roots, canonicalised, as `add_root` produced them.
const KEY_ROOTS: &str = "roots";

/// The commands [`open_in`] runs.
const KEY_OPEN_IN: &str = "openIn";

/// The view state, opaque to Rust — see [`ui`].
const KEY_UI: &str = "ui";

/// The one key in the cache file, holding a whole [`CacheFile`].
const KEY_CACHE: &str = "cache";

/// The cache shape this build understands. A file carrying anything else is ignored rather than
/// migrated: one scan rebuilds it, so the cheap answer is the right one.
const CACHE_VERSION: u32 = 1;

/// How to launch one external tool.
///
/// `{path}` in any argument is replaced with the repository's path, and appended as a final
/// argument when no argument mentions it — so `code` and `code {path}` both work, and
/// `wt.exe -d {path}` puts it where that tool needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSpec {
    /// The program to run. Empty means "not configured", which `open_in` reports as such.
    pub program: String,

    /// Its arguments. `serde(default)` so `{"program": "code"}` is a valid hand-edit.
    #[serde(default)]
    pub args: Vec<String>,
}

impl LaunchSpec {
    /// A spec with no program, which `open_in` refuses rather than guessing at.
    fn none() -> Self {
        Self {
            program: String::new(),
            args: Vec::new(),
        }
    }

    /// A program and its arguments.
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
        }
    }
}

/// The commands `open_in` runs for its two spawning targets.
///
/// The file manager is not here: it goes through `tauri-plugin-opener`'s `reveal_item_in_dir`, which
/// is the platform's own API and has nothing to configure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenInSettings {
    /// The editor. `code` is on `PATH` wherever VS Code is installed.
    pub editor: LaunchSpec,

    /// The terminal. Windows is the only platform with an answer worth defaulting to.
    pub terminal: LaunchSpec,
}

/// Windows-first defaults, because this is a Windows-first app.
///
/// `wt.exe -d` opens Windows Terminal in a directory. Elsewhere the terminal is deliberately
/// **unset** rather than guessed at: `x-terminal-emulator`, `gnome-terminal` and `open -a Terminal`
/// all want different arguments, and an unverified default that silently fails is worse than a
/// refusal naming the file to edit.
impl Default for OpenInSettings {
    fn default() -> Self {
        Self {
            editor: LaunchSpec::new("code", &["{path}"]),
            terminal: if cfg!(windows) {
                LaunchSpec::new("wt.exe", &["-d", "{path}"])
            } else {
                LaunchSpec::none()
            },
        }
    }
}

/// The persisted row cache.
///
/// Both maps, not just the rows. `RepoStatus` carries no `git_dir` and every engine entry point
/// needs the resolved one, so a cache of rows alone would paint a table whose every row refused to
/// refresh or expand until a scan had rediscovered it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    /// [`CACHE_VERSION`]. Anything else is ignored.
    version: u32,

    /// When it was written, for diagnostics. The rows carry their own ages.
    saved_at_ms: u64,

    /// The canonical rows.
    rows: Vec<RepoStatus>,

    /// What discovery found — a superset of `rows`, and where `git_dir` lives.
    found: Vec<DiscoveredRepo>,
}

/// Seed the app from disk. Called once, from `setup`, before any command can run.
///
/// Returns nothing and fails at nothing: every step is skipped with a log line if it cannot be done.
/// The roots go in first, because a cached row is only reachable through the root above it.
pub fn load<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    match roots(app) {
        Ok(roots) => {
            for root in roots {
                state.add_root(root);
            }
        }
        Err(error) => tracing::warn!(%error, "could not read the persisted roots"),
    }

    // Written back on a first run so the file documents its own shape. There is no settings screen,
    // so an empty file would leave a user with nothing to edit and no hint of what to write.
    if let Err(error) = seed_open_in(app) {
        tracing::warn!(%error, "could not write the default open-in commands");
    }

    match cache(app) {
        Ok(Some(cached)) => {
            let rows = cached.rows.into_iter().map(without_tier2).collect();
            state.restore(rows, cached.found);
        }
        Ok(None) => tracing::debug!("no usable row cache"),
        Err(error) => tracing::warn!(%error, "could not read the row cache"),
    }
}

/// The persisted roots, or an empty list when there are none.
fn roots<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<PathBuf>> {
    let Some(stored) = settings(app)?.get(KEY_ROOTS) else {
        return Ok(Vec::new());
    };
    serde_json::from_value(stored).context("the persisted roots are not a list of paths")
}

/// Persist the root list.
///
/// Best-effort: the roots are already in memory and the command that changed them has succeeded, so
/// a write failure is logged rather than returned.
pub fn save_roots<R: Runtime>(app: &AppHandle<R>, roots: &[PathBuf]) {
    if let Err(error) = write(app, KEY_ROOTS, roots) {
        tracing::warn!(%error, "could not persist the roots");
    }
}

/// The commands `open_in` runs, defaulted when they are absent or unreadable.
///
/// Read on every call rather than held in state, which is what lets a hand-edit of the file take
/// effect without restarting the app — the only way to change these, since nothing in the UI does.
pub fn open_in<R: Runtime>(app: &AppHandle<R>) -> OpenInSettings {
    let stored = settings(app).ok().and_then(|store| store.get(KEY_OPEN_IN));

    let Some(stored) = stored else {
        return OpenInSettings::default();
    };
    match serde_json::from_value(stored) {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "the open-in commands could not be read; using the defaults");
            OpenInSettings::default()
        }
    }
}

/// Write the default commands if the key is absent, so the file shows what can be set.
fn seed_open_in<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    if settings(app)?.has(KEY_OPEN_IN) {
        return Ok(());
    }
    write(app, KEY_OPEN_IN, &OpenInSettings::default())
}

/// The view state, verbatim, or `null` when nothing has been saved.
///
/// **Opaque on purpose.** Chips, sort keys and grouping are vocabulary of the table and Rust acts on
/// none of it, so it round-trips as JSON and TypeScript owns the shape in `src/scripts/settings.ts`.
/// Typing it here would mean either putting UI concepts in the engine crate — the crate that gets
/// swapped for a different domain — or a `ts-rs` derive in `src-tauri` that breaks `vp run types`'
/// scoping. The cost is that a hand-edited object can be malformed, so the frontend parses it field
/// by field with defaults.
pub fn ui<R: Runtime>(app: &AppHandle<R>) -> Result<JsonValue> {
    Ok(settings(app)?.get(KEY_UI).unwrap_or(JsonValue::Null))
}

/// Persist the view state. Fallible, because a command is waiting on it.
pub fn save_ui<R: Runtime>(app: &AppHandle<R>, value: &JsonValue) -> Result<()> {
    write(app, KEY_UI, value)
}

/// Write the row cache.
///
/// Best-effort, and never on the path of anything a user is waiting for: it runs after a scan's
/// terminal event has already gone out.
pub fn save_cache<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    if let Err(error) = try_save_cache(app, state) {
        tracing::warn!(%error, "could not write the row cache");
    }
}

/// The half of [`save_cache`] that can fail.
fn try_save_cache<R: Runtime>(app: &AppHandle<R>, state: &AppState) -> Result<()> {
    let file = CacheFile {
        version: CACHE_VERSION,
        saved_at_ms: now_ms(),
        rows: state.snapshot(),
        found: state.discovered_all(),
    };

    let store = cache_store(app)?;
    store.set(KEY_CACHE, serde_json::to_value(&file)?);
    // Explicit, because this store has no auto-save to do it. `save` also flushes a pending
    // debounce, so it stays the right call if that ever changes.
    store.save().context("writing the cache file failed")?;

    tracing::debug!(
        rows = file.rows.len(),
        found = file.found.len(),
        "row cache written"
    );
    Ok(())
}

/// The cached snapshot, or `None` when there is nothing usable to load.
fn cache<R: Runtime>(app: &AppHandle<R>) -> Result<Option<CacheFile>> {
    let Some(stored) = cache_store(app)?.get(KEY_CACHE) else {
        return Ok(None);
    };

    let file: CacheFile = serde_json::from_value(stored)
        .context("the row cache is not in a shape this build reads")?;
    if file.version != CACHE_VERSION {
        tracing::info!(
            found = file.version,
            expected = CACHE_VERSION,
            "ignoring a row cache from another version"
        );
        return Ok(None);
    }
    Ok(Some(file))
}

/// A row with its Tier 2 fields cleared — see this module's note on the cache being a past claim.
fn without_tier2(row: RepoStatus) -> RepoStatus {
    RepoStatus {
        counts: None,
        submodules: None,
        ..row
    }
}

/// The settings store, with the plugin's default auto-save.
fn settings<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<Store<R>>> {
    app.store(SETTINGS_FILE)
        .context("the settings file could not be opened")
}

/// The cache store, with auto-save **off**.
///
/// `build` returns an already-loaded store for the same path if there is one, so this must stay the
/// only place `cache.json` is opened — opening it once with the defaults would leave auto-save on
/// for the rest of the session.
fn cache_store<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<Store<R>>> {
    app.store_builder(CACHE_FILE)
        .disable_auto_save()
        .build()
        .context("the cache file could not be opened")
}

/// Serialise `value` into the settings store under `key`.
fn write<R: Runtime, T: Serialize + ?Sized>(
    app: &AppHandle<R>,
    key: &str,
    value: &T,
) -> Result<()> {
    settings(app)?.set(key, serde_json::to_value(value)?);
    Ok(())
}

/// Now, as epoch milliseconds.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use repo_scan::{FileCounts, Head, RepoKind, RepoState};

    use super::*;

    /// A minimal row, in the same shape `state.rs`'s tests use.
    fn row(path: &str) -> RepoStatus {
        RepoStatus {
            path: PathBuf::from(path),
            name: "repo".into(),
            parent: PathBuf::from("C:/work"),
            kind: RepoKind::Normal,
            head: Head::Branch {
                name: "main".into(),
            },
            upstream: None,
            ahead: None,
            behind: None,
            last_commit: None,
            stash_count: 0,
            state: RepoState::Clean,
            last_fetched_ms: None,
            dirty: None,
            conflicted: None,
            counts: None,
            submodules: None,
            scanned_at_ms: 1_000,
            error: None,
        }
    }

    /// A cached row's age is the whole reason it can be shown at all, so the round trip must not
    /// touch it — and Tier 2 must not survive it.
    #[test]
    fn a_cached_row_keeps_its_age_and_loses_its_counts() {
        let cached = RepoStatus {
            counts: Some(FileCounts::default()),
            submodules: Some(Vec::new()),
            dirty: Some(true),
            ..row("C:/work/alpha")
        };

        let loaded = without_tier2(cached.clone());

        assert_eq!(
            loaded.scanned_at_ms, cached.scanned_at_ms,
            "the age is what makes a cached row honest"
        );
        assert_eq!(loaded.dirty, Some(true), "Tier 1 is cached, age and all");
        assert_eq!(
            loaded.counts, None,
            "Tier 2 is not: the drawer shows no age beside its counts"
        );
        assert_eq!(loaded.submodules, None);
    }

    /// The file is only ever read back through `serde_json`, so the round trip is the contract.
    #[test]
    fn the_cache_file_round_trips() {
        let file = CacheFile {
            version: CACHE_VERSION,
            saved_at_ms: 1_700_000_000_000,
            rows: vec![row("C:/work/alpha")],
            found: Vec::new(),
        };

        let json = serde_json::to_value(&file).expect("serialises");
        assert_eq!(
            json["savedAtMs"], 1_700_000_000_000_u64,
            "camelCase, like everything else on the wire"
        );

        let back: CacheFile = serde_json::from_value(json).expect("deserialises");
        assert_eq!(back.rows.len(), 1);
        assert_eq!(back.rows[0].path, file.rows[0].path);
    }

    /// A cache from another build is ignored rather than half-read, and the version field is what
    /// makes that possible.
    #[test]
    fn a_cache_from_another_version_is_not_loaded() {
        let json = serde_json::json!({
            "version": CACHE_VERSION + 1,
            "savedAtMs": 1,
            "rows": [],
            "found": [],
        });

        let file: CacheFile = serde_json::from_value(json).expect("the envelope still parses");
        assert_ne!(file.version, CACHE_VERSION, "which is what `cache` checks");
    }

    /// A hand-edit is the only way to change these, so the shortest plausible one has to parse.
    #[test]
    fn a_program_with_no_arguments_is_a_valid_hand_edit() {
        let spec: LaunchSpec =
            serde_json::from_str("{\"program\":\"code\"}").expect("args default to none");

        assert_eq!(spec.program, "code");
        assert!(spec.args.is_empty());
    }

    /// Only the terminal is platform-conditional, and only Windows has a default worth shipping.
    #[test]
    fn the_defaults_name_an_editor_everywhere_and_a_terminal_on_windows() {
        let defaults = OpenInSettings::default();

        assert_eq!(defaults.editor.program, "code");
        assert_eq!(defaults.terminal.program.is_empty(), !cfg!(windows));
    }
}
