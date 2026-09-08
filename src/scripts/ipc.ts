/**
 * The IPC surface. **This is the only file in `src/` that imports `@tauri-apps/api`.**
 *
 * Components and stores go through the functions exported here. Dialog, opener, and store are
 * reached through this app's own Rust commands rather than through `@tauri-apps/plugin-*` packages,
 * which is what keeps the IPC surface auditable, `capabilities/default.json` at `core:default`, and
 * components testable with `mockIPC`.
 *
 * Command signatures are hand-written rather than generated. `tauri-specta` would type them
 * automatically but couples the engine to Tauri; there are few of them and they rarely change. The
 * _data_ types they carry are generated — see `src/scripts/generated/`, written by `vp run types`
 * from the Rust model.
 */
import { invoke } from '@tauri-apps/api/core';

/**
 * Round-trips a call to the Rust backend.
 *
 * Called on load so a broken bridge surfaces as a visible failure on the page rather than as an
 * empty screen. Resolves to `'pong'`.
 *
 * @returns The backend's reply.
 */
export function ping(): Promise<string> {
  return invoke<string>('ping');
}
