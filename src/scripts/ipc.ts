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
