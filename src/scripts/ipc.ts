/**
 * The IPC surface. **This is the only file in `src/` that imports `@tauri-apps/api`.**
 *
 * Components and stores go through the functions exported here. Dialog, opener, and store are
 * reached through this app's own Rust commands rather than through `@tauri-apps/plugin-*` packages,
 * which is what keeps the IPC surface auditable, `capabilities/default.json` at `core:default`, and
 * components testable with `mockIPC`.
 *
 * This module is **stateless**. It builds a `Channel`, wires the handler, invokes, and returns; it
 * holds no session, no active scan id, and no policy. That belongs to `scan.ts`. Statelessness is
 * what makes `clearMocks()` between tests safe with no reset hatch — and the `Channel` is never
 * returned, so no other file can hold one, re-key one, or reach `@tauri-apps/api` through it.
 *
 * Command signatures are hand-written rather than generated. `tauri-specta` would type them
 * automatically but couples the engine to Tauri; there are few of them and they rarely change. The
 * _data_ types they carry are generated — see `src/scripts/generated/`, written by `vp run types`
 * from the Rust model. Function names mirror the Rust command names one for one, which is why they
 * stay camelCase while the actions in `scan.ts` are PascalCase.
 */
import { Channel, invoke } from '@tauri-apps/api/core';
import type { RepoEvent } from '@/scripts/generated/RepoEvent';
import type { RepoStatus } from '@/scripts/generated/RepoStatus';
import type { ScanEvent } from '@/scripts/generated/ScanEvent';
import type { ScanId } from '@/scripts/generated/ScanId';
import type { ScanOpts } from '@/scripts/generated/ScanOpts';
import type { Tier } from '@/scripts/generated/Tier';
import type { UiSettings } from '@/scripts/settings';

/**
 * Where {@link openIn} can open a repository.
 *
 * Hand-mirrored from `OpenTarget` in `src-tauri/src/commands/open.rs`, like every signature in this
 * file. Three variants are not worth widening `vp run types` past the engine crate for, and a
 * mismatch fails immediately and loudly: Rust refuses to deserialise anything else.
 */
export type OpenTarget = 'fileManager' | 'editor' | 'terminal';

/**
 * Round-trips a call to the Rust backend.
 *
 * Called on load so a broken bridge surfaces as a visible failure on the page rather than as an
 * empty screen. It also separates "IPC is dead" from "`subscribe` specifically failed", which is
 * why it survives now that there is a real command surface beside it.
 *
 * @returns The backend's reply, `'pong'`.
 */
export function ping(): Promise<string> {
  return invoke<string>('ping');
}

/**
 * Opens the session channel and returns the rows Rust already holds.
 *
 * Called once, at app mount; the channel lives for the session and carries every row change that is
 * not part of a scan. The snapshot is what lets a reloaded webview repaint without rescanning.
 *
 * @param onEvent - Receives each session event.
 * @returns Every row in the canonical map, sorted by path.
 */
export function subscribe(onEvent: (event: RepoEvent) => void): Promise<RepoStatus[]> {
  const channel = new Channel<RepoEvent>(onEvent);
  return invoke<RepoStatus[]>('subscribe', { onEvent: channel });
}

/**
 * Starts a scan over the given roots.
 *
 * Resolves with the scan's id — but note that Rust may already have sent events on the channel by
 * then, so a caller must not use the resolved id to decide whether to accept them. See `scan.ts`.
 *
 * @param roots - Configured roots to walk. Rust rejects anything else.
 * @param opts - Walk options; every field has a Rust-side default, so a subset is fine.
 * @param onEvent - Receives each scan event.
 * @returns The id of the scan that was started.
 */
export function scanRoots(
  roots: string[],
  opts: Partial<ScanOpts>,
  onEvent: (event: ScanEvent) => void,
): Promise<ScanId> {
  const channel = new Channel<ScanEvent>(onEvent);
  return invoke<ScanId>('scan_roots', { roots, opts, onEvent: channel });
}

/**
 * Asks Rust to stop a scan.
 *
 * Cancelling a scan that has already finished is a no-op rather than a failure — that race cannot
 * be avoided from this side.
 *
 * @param id - The scan to stop.
 */
export function cancelScan(id: ScanId): Promise<void> {
  return invoke<void>('cancel_scan', { id });
}

/**
 * Reads Tier 2 for one repository — the full file counts and the submodule list.
 *
 * Resolves with the **whole merged row**, not a Tier 2 payload of its own: Rust owns the canonical
 * copy of `counts` and `submodules`, so what comes back is mirrored exactly like a scan batch. That
 * is what makes an expanded-then-collapsed row keep its counts without this side caching anything.
 *
 * Rejects when the read failed, which is where a Tier 2 failure is reported: the row's one `error`
 * slot already has two writers, and this call can be repeated once per expand.
 *
 * @param path - The repository to read. Rust accepts only a key of its canonical map.
 * @returns The merged row.
 */
export function fullStatus(path: string): Promise<RepoStatus> {
  return invoke<RepoStatus>('full_status', { path });
}

/**
 * Re-reads one repository up to and including `tier`.
 *
 * `tier` is cumulative — `'two'` reads all three — because the tiers are not independent: a fresh
 * dirty flag beside a stale branch would describe two different moments.
 *
 * The merged row also arrives on the session channel, so a caller that only wants the store updated
 * can ignore what this resolves with.
 *
 * @param path - The repository to re-read.
 * @param tier - How much of it to read.
 * @returns The merged row.
 */
export function refreshRepo(path: string, tier: Tier): Promise<RepoStatus> {
  return invoke<RepoStatus>('refresh_repo', { path, tier });
}

/**
 * Shows the native folder picker.
 *
 * @returns The chosen folder, or `null` if the user cancelled.
 */
export function pickRoot(): Promise<string | null> {
  return invoke<string | null>('pick_root');
}

/**
 * Adds a root. Rust canonicalises and validates it.
 *
 * @param path - The folder to add.
 * @returns The new root list, so this side never maintains its own copy.
 */
export function addRoot(path: string): Promise<string[]> {
  return invoke<string[]>('add_root', { path });
}

/**
 * Removes a root and evicts its rows. The evicted paths arrive on the session channel.
 *
 * @param path - The folder to remove.
 * @returns The new root list.
 */
export function removeRoot(path: string): Promise<string[]> {
  return invoke<string[]>('remove_root', { path });
}

/**
 * The configured roots.
 *
 * @returns The root list.
 */
export function listRoots(): Promise<string[]> {
  return invoke<string[]>('list_roots');
}

/**
 * Opens one repository in an external tool.
 *
 * Rust validates the path against what discovery found — a superset of the rows — so this works for
 * a repository whose HEAD could not be read, which is exactly the one a user wants to go and look
 * at. It resolves when the tool has been _started_; nothing waits on what the tool then does.
 *
 * @param path - The repository to open.
 * @param target - Where to open it.
 */
export function openIn(path: string, target: OpenTarget): Promise<void> {
  return invoke<void>('open_in', { path, target });
}

/**
 * The persisted view state, or `null` when nothing has been saved.
 *
 * Typed as `unknown` deliberately. This is the one value crossing the boundary that `ts-rs` does
 * not generate — see `settings.ts` — so it arrives unvalidated and `parseUiSettings` is what turns
 * it into a `UiSettings`. Claiming the type here would move the lie one file earlier.
 *
 * @returns Whatever the settings file held.
 */
export function uiSettings(): Promise<unknown> {
  return invoke<unknown>('ui_settings');
}

/**
 * Persists the view state.
 *
 * @param ui - The settings to save.
 */
export function saveUiSettings(ui: UiSettings): Promise<void> {
  return invoke<void>('save_ui_settings', { ui });
}
