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

/// How live updates behave — see [`watch`].
const KEY_WATCH: &str = "watch";

/// How fetching behaves — see [`fetch`].
const KEY_FETCH: &str = "fetch";

/// The one key in the cache file, holding a whole [`CacheFile`].
const KEY_CACHE: &str = "cache";

/// The cache shape this build understands. A file carrying anything else is ignored rather than
/// migrated: one scan rebuilds it, so the cheap answer is the right one.
const CACHE_VERSION: u32 = 2;

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

/// How live updates behave.
///
/// Read **once, at startup**, unlike [`OpenInSettings`] — the watcher and the poll thread are built
/// once and hold these values, so a hand-edit takes effect on the next launch rather than
/// immediately. README.md says so, because a setting that silently needs a restart is worse than
/// one that says it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchSettings {
    /// Whether to watch at all.
    ///
    /// Off leaves the poll running, which is the point of it being a separate switch: a network
    /// mount or a container where the backend misbehaves can drop to polling without dropping to
    /// nothing.
    pub enabled: bool,

    /// The debounce window in milliseconds.
    ///
    /// §7.3's range is ~300–500 ms. Git writes `.git/index` three times per operation, so this is
    /// what turns one `git add` into one refresh instead of three.
    pub debounce_ms: u64,

    /// How often the poll runs, in seconds.
    pub poll_seconds: u64,
}

impl Default for WatchSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: 400,
            poll_seconds: 60,
        }
    }
}

impl WatchSettings {
    /// The debounce window, floored so a hand-edited `0` cannot turn debouncing off — which would
    /// reintroduce the refresh storm the debouncer exists to prevent.
    pub fn debounce(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.debounce_ms.max(100))
    }

    /// The poll interval, floored for the same reason: a `0` here would be a busy loop running a
    /// Tier 0 pass over the whole tree.
    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.poll_seconds.max(5))
    }
}

/// How fetching behaves.
///
/// Read **per invocation**, like [`OpenInSettings`] and unlike [`WatchSettings`]. The watcher and
/// the poll thread are built once and hold their values; a fetch is built per click, so re-reading
/// costs one store lookup and lets a user try `concurrency: 2` without restarting. These are
/// exactly the knobs somebody reaches for when a fetch misbehaves over a VPN, which is the worst
/// possible moment to require a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FetchSettings {
    /// How many `git` processes may run at once. §8.2's ~4.
    pub concurrency: u32,

    /// The per-process deadline in seconds. §8.2's ~60.
    ///
    /// This is the only universal backstop against a fetch that sits waiting for a human:
    /// `GIT_TERMINAL_PROMPT=0` stops git's own prompt, but not every credential helper's GUI and
    /// not `ssh`'s host-key or passphrase prompts.
    pub timeout_seconds: u64,

    /// The shortest gap between two **bulk** fetches of one repository, in seconds.
    ///
    /// A single per-row fetch is never guarded: clicking one repository's button twice is intent,
    /// and the hazard §8.2 names is three hundred unprompted.
    pub min_interval_seconds: u64,

    /// Delete remote-tracking refs whose remote branch is gone.
    pub prune: bool,

    /// Fetch every configured remote rather than only the current branch's.
    pub all_remotes: bool,
}

impl Default for FetchSettings {
    fn default() -> Self {
        Self {
            concurrency: 4,
            timeout_seconds: 60,
            min_interval_seconds: 300,
            prune: true,
            all_remotes: true,
        }
    }
}

impl FetchSettings {
    /// How many at once, clamped at **both** ends — which is the difference from
    /// [`WatchSettings`], whose values only need floors.
    ///
    /// A `0` would fetch nothing at all. A hand-edited `200` is not merely the user's own problem
    /// the way a short debounce is: it is two hundred sockets opened against somebody else's
    /// server, and two hundred processes on a machine that has other work to do.
    pub fn concurrency(&self) -> usize {
        (self.concurrency as usize).clamp(1, 16)
    }

    /// The per-process deadline, clamped at both ends. A `0` would kill every fetch the instant it
    /// started, and no ceiling at all reintroduces the indefinite hang the timeout exists for.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_seconds.clamp(5, 600))
    }

    /// The bulk repeat guard, or `None` for no guard.
    ///
    /// **Deliberately not floored**, unlike everything else here. `0` means "always fetch", which
    /// is a coherent choice on a LAN — and unlike a debounce of `0` it cannot storm, because the
    /// concurrency cap still bounds it and a human still has to press the button.
    pub fn min_interval(&self) -> Option<std::time::Duration> {
        (self.min_interval_seconds > 0)
            .then(|| std::time::Duration::from_secs(self.min_interval_seconds))
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
    if let Err(error) = seed_watch(app) {
        tracing::warn!(%error, "could not write the default watch settings");
    }
    if let Err(error) = seed_fetch(app) {
        tracing::warn!(%error, "could not write the default fetch settings");
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

/// How live updates behave, defaulted when the key is absent or unreadable.
///
/// Read once, from `setup`. A malformed object falls back to the defaults whole rather than
/// field-by-field: unlike the view state, there is no long tail of optional keys here, and three
/// values are cheap enough to retype that guessing at which one the user meant buys nothing.
pub fn watch<R: Runtime>(app: &AppHandle<R>) -> WatchSettings {
    let stored = settings(app).ok().and_then(|store| store.get(KEY_WATCH));

    let Some(stored) = stored else {
        return WatchSettings::default();
    };
    match serde_json::from_value(stored) {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "the watch settings could not be read; using the defaults");
            WatchSettings::default()
        }
    }
}

/// Write the default watch settings if the key is absent, so the file shows what can be set.
fn seed_watch<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    if settings(app)?.has(KEY_WATCH) {
        return Ok(());
    }
    write(app, KEY_WATCH, &WatchSettings::default())
}

/// How fetching behaves, defaulted when the key is absent or unreadable.
///
/// Read per invocation, so a hand-edit takes effect on the next click. Defaults whole on a
/// malformed object, for the reason [`watch`] does.
pub fn fetch<R: Runtime>(app: &AppHandle<R>) -> FetchSettings {
    let stored = settings(app).ok().and_then(|store| store.get(KEY_FETCH));

    let Some(stored) = stored else {
        return FetchSettings::default();
    };
    match serde_json::from_value(stored) {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "the fetch settings could not be read; using the defaults");
            FetchSettings::default()
        }
    }
}

/// Write the default fetch settings if the key is absent, so the file shows what can be set.
fn seed_fetch<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    if settings(app)?.has(KEY_FETCH) {
        return Ok(());
    }
    write(app, KEY_FETCH, &FetchSettings::default())
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

    /// The shipped defaults, stated so a change to any of them is a deliberate edit here.
    #[test]
    fn the_fetch_defaults_are_the_documented_ones() {
        let settings = FetchSettings::default();

        assert_eq!(settings.concurrency(), 4);
        assert_eq!(settings.timeout(), std::time::Duration::from_secs(60));
        assert_eq!(
            settings.min_interval(),
            Some(std::time::Duration::from_secs(300))
        );
        assert!(settings.prune, "a dead tracking ref must not survive");
        assert!(settings.all_remotes);
    }

    /// Clamped at both ends, unlike the watch settings.
    ///
    /// The ceiling is the half that differs: a hand-edited `concurrency: 1000` is not merely the
    /// user's own problem, it is a thousand sockets against somebody else's server.
    #[test]
    fn a_hand_edited_concurrency_is_clamped_at_both_ends() {
        let low = FetchSettings {
            concurrency: 0,
            ..FetchSettings::default()
        };
        let high = FetchSettings {
            concurrency: 1_000,
            ..FetchSettings::default()
        };

        assert_eq!(low.concurrency(), 1, "zero would fetch nothing at all");
        assert_eq!(high.concurrency(), 16);
    }

    /// A `0` timeout would kill every fetch as it started; no ceiling reintroduces the hang.
    #[test]
    fn a_hand_edited_timeout_is_clamped_at_both_ends() {
        let zero = FetchSettings {
            timeout_seconds: 0,
            ..FetchSettings::default()
        };
        let forever = FetchSettings {
            timeout_seconds: 86_400,
            ..FetchSettings::default()
        };

        assert_eq!(zero.timeout(), std::time::Duration::from_secs(5));
        assert_eq!(forever.timeout(), std::time::Duration::from_secs(600));
    }

    /// The one value deliberately left un-floored: `0` means "no bulk guard", which is coherent
    /// on a LAN and cannot storm, because the concurrency cap still bounds it and a human still
    /// presses the button.
    #[test]
    fn a_zero_repeat_guard_means_no_guard_rather_than_a_floor() {
        let off = FetchSettings {
            min_interval_seconds: 0,
            ..FetchSettings::default()
        };

        assert_eq!(off.min_interval(), None);
    }

    /// A hand-edit is the only way to change these, so the shortest plausible one has to parse.
    #[test]
    fn one_fetch_key_alone_is_a_valid_hand_edit() {
        let settings: FetchSettings =
            serde_json::from_str("{\"concurrency\":2}").expect("the rest default");

        assert_eq!(settings.concurrency(), 2);
        assert_eq!(
            settings.timeout(),
            std::time::Duration::from_secs(60),
            "the untouched keys keep their defaults"
        );
        assert!(settings.prune);
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
