import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import type { DiscoveredRepo } from '@/scripts/generated/DiscoveredRepo';
import type { RepoStatus } from '@/scripts/generated/RepoStatus';
import type { ScanError } from '@/scripts/generated/ScanError';
import type { ScanSummary } from '@/scripts/generated/ScanSummary';
import type { Tier0Summary } from '@/scripts/generated/Tier0Summary';

/**
 * One row of the table: either a repository Rust has read, or one it has only found.
 *
 * These are the two things Rust can honestly say about a repository, and they are different types
 * rather than one type with optional halves. A `DiscoveredRepo` has no `head` and no `state`
 * because discovery has not read them; inventing them would be the lie the tiering exists to
 * avoid.
 */
export type RepoRow = DiscoveredRepo | RepoStatus;

/**
 * How far a scan has got. A boolean could not express `cancelled`.
 */
export type ScanPhase = 'idle' | 'discovering' | 'reading' | 'done' | 'cancelled' | 'failed';

/**
 * What the progress indicator needs, in one object.
 */
export type ScanProgress = {
  /**
   * The phase the scan is in.
   */
  phase: ScanPhase;
  /**
   * Repositories the walk has reported.
   */
  found: number;
  /**
   * Rows Tier 0 has produced.
   */
  read: number;
  /**
   * The final repository count, or `null` until the walk finishes.
   *
   * `null` and never `0`: an unknown denominator renders as an indeterminate bar, where `0` would
   * render as a full one. The uncomputed-is-not-zero rule applied to progress itself.
   */
  total: number | null;
};

/**
 * Narrows a row to one Rust has actually read.
 *
 * A structural test on the generated types, inventing nothing: `head` exists on `RepoStatus` and
 * not on `DiscoveredRepo`. The payoff is that a discovered-only row cannot reach `.dirty` at all —
 * the property does not exist on it, so the compiler stops the mistake instead of a runtime check
 * having to.
 *
 * @param row - The row to narrow.
 * @returns Whether Tier 0 has produced this row.
 */
export function isRead(row: RepoRow): row is RepoStatus {
  return 'head' in row;
}

/**
 * The repository rows, mirrored from Rust.
 *
 * **This store is a mirror, not a model.** Rust owns the canonical state: `src-tauri/src/state.rs`
 * holds the one `HashMap<PathBuf, RepoStatus>`, merges each tier into it, and sends the full merged
 * row. This store keys those rows by path and replaces them wholesale.
 *
 * It therefore never merges tiers, never infers a value, and never holds a value Rust does not.
 * Doing any of those reintroduces exactly the bug the Rust-side merge exists to prevent: a Tier 0
 * result arriving after a Tier 1 result carries `dirty: null`, and a store that "helpfully" filled
 * that in would show a stale answer as a fresh one.
 *
 * The scan-session fields — `phase`, the summaries, `scanError` — are not row values and are not
 * covered by that rule. They are this side's bookkeeping over events Rust sent, which is the
 * frontend's own business.
 *
 * Uncomputed fields stay `null` and must render as unknown — never as `0`.
 */
export const useReposStore = defineStore('repos', () => {
  /// Data
  /**
   * Rows by absolute path. The key is the same string Rust uses as its map key, so it is also the
   * only value a command taking a path will accept.
   */
  const byPath = ref(new Map<string, RepoRow>());

  /**
   * How far the current scan has got.
   */
  const phase = ref<ScanPhase>('idle');

  /**
   * What the walk did, once it has finished. `null` while it is still running.
   */
  const discovery = ref<ScanSummary | null>(null);

  /**
   * What Tier 0 did, once the scan has finished. `null` until then.
   */
  const tier0 = ref<Tier0Summary | null>(null);

  /**
   * A failed command — not a per-repository failure, which rides on the row itself.
   */
  const scanError = ref<string | null>(null);

  /**
   * The configured roots, mirrored from Rust exactly as the rows are.
   */
  const roots = ref<string[]>([]);

  /// Computed
  /**
   * Every row, ordered by folder then name.
   *
   * The parallel walk emits in an arbitrary order, so unsorted rows look random. This is
   * presentation ordering only; Phase 5's sort store replaces it.
   */
  const rows = computed<RepoRow[]>(() =>
    [...byPath.value.values()].toSorted(
      (left, right) =>
        left.parent.localeCompare(right.parent) || left.name.localeCompare(right.name),
    ),
  );

  /**
   * How many rows are known.
   */
  const count = computed(() => byPath.value.size);

  /**
   * How many rows Tier 0 has produced.
   */
  const readCount = computed(() => rows.value.filter((row) => isRead(row)).length);

  /**
   * True while a scan is in flight.
   */
  const scanning = computed(() => phase.value === 'discovering' || phase.value === 'reading');

  /**
   * Everything the progress indicator renders.
   */
  const progress = computed<ScanProgress>(() => ({
    phase: phase.value,
    found: byPath.value.size,
    read: readCount.value,
    total: discovery.value?.reposFound ?? null,
  }));

  /**
   * Tier 0 failures by path, or `null` until Tier 0 has finished.
   *
   * `null` rather than an empty map, for the same reason `total` is `null` rather than `0`: "Tier 0
   * has not finished" and "Tier 0 finished and found nothing wrong" are different facts, and a row
   * with no status yet means something different in each.
   */
  const readErrors = computed<Map<string, string> | null>(() => {
    if (tier0.value === null) return null;
    return new Map(tier0.value.errors.map((error: ScanError) => [error.path, error.message]));
  });

  /// Methods
  /**
   * Replaces one row with the version Rust sent.
   *
   * Wholesale replacement is the point — see the note above. Never merge here. A `RepoStatus`
   * landing on a path that currently holds a `DiscoveredRepo` replaces it in place: `Map.set` keeps
   * the existing key's position, so the row does not jump as it fills in.
   *
   * @param row - The full row, as Rust sent it.
   */
  function Upsert(row: RepoRow): void {
    byPath.value.set(row.path, row);
  }

  /**
   * Replaces a batch of rows. Rows arrive batched, so this is the common path.
   *
   * @param batch - The rows to replace.
   */
  function UpsertMany(batch: RepoRow[]): void {
    for (const row of batch) byPath.value.set(row.path, row);
  }

  /**
   * Drops rows by path, for a root the user removed.
   *
   * @param paths - Absolute paths, as Rust spells them.
   */
  function Remove(paths: string[]): void {
    for (const path of paths) byPath.value.delete(path);
  }

  /**
   * Drops every row, keeping the scan summaries.
   */
  function Clear(): void {
    byPath.value.clear();
  }

  /**
   * Drops every row and every summary, for a fresh scan.
   */
  function Reset(): void {
    byPath.value.clear();
    discovery.value = null;
    tier0.value = null;
    scanError.value = null;
  }

  /**
   * Records how far the scan has got.
   *
   * @param next - The new phase.
   */
  function SetPhase(next: ScanPhase): void {
    phase.value = next;
  }

  /**
   * Records what the walk did.
   *
   * @param summary - The walk's summary, or `null` to clear it.
   */
  function SetDiscoverySummary(summary: ScanSummary | null): void {
    discovery.value = summary;
  }

  /**
   * Records what Tier 0 did.
   *
   * @param summary - Tier 0's summary, or `null` to clear it.
   */
  function SetTier0Summary(summary: Tier0Summary | null): void {
    tier0.value = summary;
  }

  /**
   * Records a command failure.
   *
   * @param message - The failure, or `null` to clear it.
   */
  function SetScanError(message: string | null): void {
    scanError.value = message;
  }

  /**
   * Mirrors the root list Rust returned.
   *
   * @param list - The roots, as Rust spells them.
   */
  function SetRoots(list: string[]): void {
    roots.value = list;
  }

  return {
    byPath,
    phase,
    discovery,
    tier0,
    scanError,
    roots,
    rows,
    count,
    readCount,
    scanning,
    progress,
    readErrors,
    Upsert,
    UpsertMany,
    Remove,
    Clear,
    Reset,
    SetPhase,
    SetDiscoverySummary,
    SetTier0Summary,
    SetScanError,
    SetRoots,
  };
});
