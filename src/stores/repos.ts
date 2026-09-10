import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import type { DiscoveredRepo } from '@/scripts/generated/DiscoveredRepo';
import type { RepoStatus } from '@/scripts/generated/RepoStatus';
import type { ScanError } from '@/scripts/generated/ScanError';
import type { ScanSummary } from '@/scripts/generated/ScanSummary';
import type { ScanTotals } from '@/scripts/generated/ScanTotals';

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
 * frontend's own business. So is which rows are expanded: that is a fact about this window, not
 * about a repository, and Rust neither knows nor needs to.
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
   * What the whole scan did, tier by tier, once it has finished. `null` until then.
   *
   * Doubles as the "Tier 0 has finished" signal — see {@link tier0Done}.
   */
  const totals = ref<ScanTotals | null>(null);

  /**
   * Repositories that produced no row at all, by path, with the cause.
   *
   * Accumulated from `RepoErrors` events as they arrive rather than derived from the terminal one,
   * because a scan is not quick: at Tier 1's cold cost a large tree runs for the better part of a
   * minute, and a row whose HEAD could not be read must stop claiming to be "counting…" as soon as
   * Rust knows it never will be.
   *
   * Always a map, never `null`. "Has Tier 0 finished?" is a separate question and {@link tier0Done}
   * answers it: an absent entry here means nothing has failed for that path _yet_, which is not the
   * same as the scan being over. Conflating the two makes a mid-scan failure impossible to
   * express.
   */
  const repoErrors = ref(new Map<string, string>());

  /**
   * Which rows are expanded, by path.
   *
   * Window state, not row state: it is not mirrored from Rust and Rust is not told about it. A
   * `Set` because the only questions asked of it are membership and toggling.
   */
  const expanded = ref(new Set<string>());

  /**
   * Which rows have a Tier 2 read in flight, by path.
   *
   * What makes the drawer say `counting…` honestly — the claim is true exactly while a path is in
   * here, which is the transience the four-absences rule demands of that word.
   */
  const loadingDetail = ref(new Set<string>());

  /**
   * Why a row's Tier 2 read failed, by path.
   *
   * Tier 2 failures live here rather than on `RepoStatus.error`: the row's one error slot is owned
   * by the tiers that run during a scan, and this read can be repeated once per expand, so writing
   * there would either stack messages or erase an earlier tier's cause.
   */
  const detailErrors = ref(new Map<string, string>());

  /**
   * Why a row's last open-in launch failed, by path.
   *
   * A third error map rather than a reuse of either of the others, for the reason the row itself
   * has only one `error` slot and a rule about who may write it: a failed launch is not a failed
   * read, and it is not a failed command in the page-wide sense either. It belongs to one row and
   * one button, and it is cleared by the next attempt.
   */
  const openErrors = ref(new Map<string, string>());

  /**
   * A failed command — not a per-repository failure, which rides on the row itself.
   */
  const scanError = ref<string | null>(null);

  /**
   * Why live updates are degraded, or `null` when they are not.
   *
   * Separate from {@link scanError} because it is not a failed command and not fatal: the poll and
   * the refresh-on-focus still run, so this is the difference between rows updating in a second and
   * updating within a minute. Presenting it as an error would overstate it, and hiding it would
   * leave a user wondering why the table went quiet.
   */
  const watchError = ref<string | null>(null);

  /**
   * The configured roots, mirrored from Rust exactly as the rows are.
   */
  const roots = ref<string[]>([]);

  /// Computed
  /**
   * Every row, ordered by folder then name.
   *
   * The parallel walk emits in an arbitrary order, so unsorted rows look random. This is the base
   * order the table is built from rather than the order it renders: `view.ts` sorts by the column
   * the user chose, and falls back to exactly this pair when that column has nothing to compare.
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
   * Whether Tier 0 has finished, which is what turns a row's "not yet" into "never".
   *
   * Kept separate from {@link repoErrors} rather than inferred from it, because `RepoErrors` arrives
   * per batch: a row can be known-unreadable while the scan is still running, so "an error arrived
   * for this path" and "Tier 0 is done" are different questions. A row still lacking a status once
   * this is `true` will never get one.
   */
  const tier0Done = computed(() => totals.value !== null);

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
   * Drops everything the previous scan reported, keeping the rows.
   *
   * What a **reconciling** scan needs — the one at launch, over rows restored from the cache. Those
   * rows are what the window is painting, so clearing them would make them flash and vanish, which
   * is worse than never having cached them. The scan overwrites each row as it re-reads it, and
   * Rust evicts the ones it does not find.
   *
   * `expanded` survives on purpose: it describes what the user has open, and a rescan of the same
   * tree should not collapse their drawers. Everything derived from the _previous_ scan's reads
   * goes, including the Tier 2 failures — a fresh scan is a fresh chance for that read to work.
   */
  function ResetSummaries(): void {
    discovery.value = null;
    totals.value = null;
    scanError.value = null;
    // A completed scan re-syncs the watch set and pushes a fresh failure if there still is one, so
    // the message describes the last sync rather than accumulating across them. Left standing it
    // would outlive the condition — a repository on a share that has since reconnected would keep
    // reporting as unwatched for the life of the session.
    watchError.value = null;
    repoErrors.value.clear();
    loadingDetail.value.clear();
    detailErrors.value.clear();
    openErrors.value.clear();
  }

  /**
   * Drops every row and every summary, for a scan that starts over.
   */
  function Reset(): void {
    byPath.value.clear();
    ResetSummaries();
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
   * Records what the whole scan did, tier by tier.
   *
   * @param summary - The scan's totals, or `null` to clear them.
   */
  function SetTotals(summary: ScanTotals | null): void {
    totals.value = summary;
  }

  /**
   * Records repositories that produced no row, as Rust reports them.
   *
   * Additive: these arrive per batch, and the terminal event repeats the complete list so a webview
   * that reloaded mid-scan is not left without them. Re-recording the same path is therefore normal
   * and overwrites rather than duplicating.
   *
   * @param errors - The failures Rust sent.
   */
  function AddRepoErrors(errors: ScanError[]): void {
    for (const error of errors) repoErrors.value.set(error.path, error.message);
  }

  /**
   * Toggles a row's drawer.
   *
   * @param path - The row to toggle.
   * @returns Whether it is now expanded.
   */
  function ToggleExpanded(path: string): boolean {
    if (expanded.value.delete(path)) return false;
    expanded.value.add(path);
    return true;
  }

  /**
   * Records whether a Tier 2 read is in flight for a row.
   *
   * @param path - The row being read.
   * @param loading - Whether the read is running.
   */
  function SetLoadingDetail(path: string, loading: boolean): void {
    if (loading) loadingDetail.value.add(path);
    else loadingDetail.value.delete(path);
  }

  /**
   * Records why a row's Tier 2 read failed, or clears it.
   *
   * @param path - The row that was read.
   * @param message - The failure, or `null` on success.
   */
  function SetDetailError(path: string, message: string | null): void {
    if (message === null) detailErrors.value.delete(path);
    else detailErrors.value.set(path, message);
  }

  /**
   * Records why a row's open-in launch failed, or clears it.
   *
   * @param path - The row whose button was pressed.
   * @param message - The failure, or `null` to clear it.
   */
  function SetOpenError(path: string, message: string | null): void {
    if (message === null) openErrors.value.delete(path);
    else openErrors.value.set(path, message);
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
   * Records why live updates are degraded, or clears it.
   *
   * @param message - The cause, or `null` to clear it.
   */
  function SetWatchError(message: string | null): void {
    watchError.value = message;
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
    totals,
    repoErrors,
    expanded,
    loadingDetail,
    detailErrors,
    openErrors,
    scanError,
    watchError,
    roots,
    rows,
    count,
    readCount,
    scanning,
    progress,
    tier0Done,
    Upsert,
    UpsertMany,
    Remove,
    Clear,
    Reset,
    ResetSummaries,
    SetPhase,
    SetDiscoverySummary,
    SetTotals,
    AddRepoErrors,
    ToggleExpanded,
    SetLoadingDetail,
    SetDetailError,
    SetOpenError,
    SetScanError,
    SetWatchError,
    SetRoots,
  };
});
