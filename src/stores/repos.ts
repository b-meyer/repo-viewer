import { defineStore } from 'pinia';
import { computed, ref } from 'vue';

/**
 * The repository rows, mirrored from Rust.
 *
 * **This store is a mirror, not a model.** Rust owns the canonical state: `src-tauri/src/state.rs`
 * holds the one `HashMap<PathBuf, RepoStatus>`, merges each tier into it field-wise, and sends the
 * full merged row over the session channel. This store keys those rows by path and replaces them
 * wholesale.
 *
 * It therefore never merges tiers, never infers a value, and never holds a value Rust does not.
 * Doing any of those reintroduces exactly the bug the Rust-side merge exists to prevent: a Tier 0
 * result arriving after a Tier 1 result carries `dirty: null`, and a store that "helpfully" filled
 * that in would show a stale answer as a fresh one.
 *
 * Uncomputed fields stay `null` and must render as unknown — never as `0`.
 */
export const useReposStore = defineStore('repos', () => {
  /// Data
  /**
   * Rows by absolute path. The key is the same string Rust uses as its map key, so it is also the
   * only value a command taking a path will accept.
   */
  const byPath = ref(new Map<string, unknown>());

  /**
   * True while a scan is in flight.
   */
  const scanning = ref(false);

  /// Computed
  /**
   * Every row, in insertion order.
   */
  const rows = computed(() => [...byPath.value.values()]);

  /**
   * How many rows are known.
   */
  const count = computed(() => byPath.value.size);

  /// Methods
  /**
   * Replaces one row with the merged version Rust sent.
   *
   * Wholesale replacement is the point — see the note above. Never merge here.
   *
   * @param path - Absolute path, as Rust spells it.
   * @param row - The full merged row.
   */
  function Upsert(path: string, row: unknown): void {
    byPath.value.set(path, row);
  }

  /**
   * Drops every row. For a root change or a fresh scan.
   */
  function Clear(): void {
    byPath.value.clear();
  }

  return { byPath, scanning, rows, count, Upsert, Clear };
});
