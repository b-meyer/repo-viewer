/**
 * Filtering, sorting and grouping — the whole of what the table shows, as pure functions.
 *
 * Nothing here reaches a store, a command, or the clock: rows, options, a time, and a set of search
 * hits go in; a view model comes out. That is what makes every rule below testable, and the rules
 * are the point of the module.
 *
 * # A chip that hides an uncounted row must say so
 *
 * `dirty` is `null` until Tier 1 runs, and a filter is the one place where that absence turns back
 * into a lie: excluding those rows silently makes them read as clean. So a chip has **three**
 * answers per row — matched, not matched, and not yet knowable — and the rows in the third group
 * are counted and reported. `RepoView.pending` is that count, and the table shows it.
 *
 * A row that Tier 0 never produced is not filtered at all. It has no field to judge and its whole
 * story is that something is wrong with it, so hiding it behind a chip would hide the one row most
 * worth looking at.
 *
 * # Unknown sorts last in both directions
 *
 * Reversing a sort must not promote every uncomputed value to the top. `null` is not "small", it is
 * "no answer", so it goes to the end whichever way the arrow points — the ordering form of never
 * rendering an uncomputed value as `0`.
 *
 * # The two lookups are exhaustive by type
 *
 * {@link CHIP_RULE} and {@link SORT_VALUE} are `Record`s keyed by the union rather than `switch`
 * statements. A missing key is a compile error, which is the same discipline as mapping
 * `gix::state::InProgress` with no wildcard arm: adding a chip or a sortable column has to be
 * answered here rather than falling through to "does not match" or "no value".
 */
import type { RepoStatus } from '@/scripts/generated/RepoStatus';
import type { FilterChip, SortDirection, SortKey, UiSettings } from '@/scripts/settings';
import { fetchStaleness } from '@/scripts/utils';
import { isRead, type RepoRow } from '@/stores/repos';

/**
 * What one chip can say about one row.
 */
type Verdict = 'match' | 'no' | 'pending';

/**
 * Anything two rows can be ordered by, or `null` for no answer.
 */
type Sortable = string | number | boolean | null;

/**
 * One block of rows in the table.
 */
export type RepoGroup = {
  /**
   * The containing folder, or `''` when grouping is off and there is only one block.
   */
  key: string;
  /**
   * Its rows, in the sort order.
   */
  rows: RepoRow[];
};

/**
 * Everything the table renders, and everything it says about what it is not rendering.
 */
export type RepoView = {
  /**
   * The rows to show, in blocks.
   */
  groups: RepoGroup[];
  /**
   * How many rows are shown.
   */
  visible: number;
  /**
   * How many are not, for any reason.
   */
  hidden: number;
  /**
   * How many of the hidden ones a chip could not yet judge — rows whose tier has not run.
   *
   * Reported rather than folded into `hidden`, because "does not match" and "cannot be asked yet"
   * are different facts, and the second one resolves itself as the scan proceeds.
   */
  pending: number;
};

/**
 * What each chip says about a row Tier 0 has read.
 *
 * Only the Tier 1 chips can answer `pending`, and only for a repository that has a worktree. A bare
 * repository can never be dirty or conflicted, so its answer is `no` and final — the same
 * distinction `AppUnknown` draws between `pending` and `na`.
 *
 * The Tier 0 chips are never pending: Tier 0 is what produces the row, so by the time there is
 * anything to judge it has already run. A `null` ahead means "no upstream to be ahead of", which is
 * an answer.
 */
const CHIP_RULE: Record<FilterChip, (row: RepoStatus, now: number) => Verdict> = {
  dirty: (row) => {
    if (row.kind === 'bare') return 'no';
    if (row.dirty === null) return 'pending';
    return row.dirty ? 'match' : 'no';
  },
  conflicted: (row) => {
    if (row.kind === 'bare') return 'no';
    if (row.conflicted === null) return 'pending';
    return row.conflicted > 0 ? 'match' : 'no';
  },
  unpushed: (row) => (row.ahead !== null && row.ahead > 0 ? 'match' : 'no'),
  detached: (row) => (row.head.kind === 'detached' ? 'match' : 'no'),
  // `never` counts: ahead and behind measured against a remote ref that was never fetched is the
  // most misleading pair the app can show, which is exactly what this chip is for.
  staleFetch: (row, now) => (fetchStaleness(row.lastFetchedMs, now) === 'fresh' ? 'no' : 'match'),
};

/**
 * What each column compares by, or `null` when the row has no answer to compare.
 *
 * `sync` sorts by how far ahead a row is, because "what have I not pushed?" is the question that
 * column exists to answer; a row with no upstream has no answer and sorts last. `state` sorts by
 * its own name, which groups every rebase together and is at least predictable.
 */
const SORT_VALUE: Record<SortKey, (row: RepoStatus) => Sortable> = {
  name: (row) => row.name,
  head: (row) => {
    if (row.head.kind === 'branch') return row.head.name;
    return row.head.kind === 'detached' ? row.head.id : null;
  },
  sync: (row) => row.ahead,
  state: (row) => row.state,
  stash: (row) => row.stashCount,
  worktree: (row) => row.dirty,
  read: (row) => row.scannedAtMs,
};

/**
 * Applies the chips, the search, the sort and the grouping, in that order.
 *
 * @param rows - Every row the store holds.
 * @param options - The active view state.
 * @param now - The current time, for the fetch-age chip.
 * @param matches - Paths the search matched, or `null` when there is no query.
 * @returns What to render, and what was left out.
 */
export function buildView(
  rows: RepoRow[],
  options: UiSettings,
  now: number,
  matches: Set<string> | null,
): RepoView {
  const kept: RepoRow[] = [];
  let pending = 0;

  for (const row of rows) {
    if (matches !== null && !matches.has(row.path)) continue;

    const verdict = judge(row, options.chips, now);
    if (verdict === 'match') kept.push(row);
    else if (verdict === 'pending') pending += 1;
  }

  kept.sort((left, right) => compareRows(left, right, options.sortKey, options.sortDirection));

  return {
    groups: options.groupByFolder ? group(kept) : [{ key: '', rows: kept }],
    visible: kept.length,
    hidden: rows.length - kept.length,
    pending,
  };
}

/**
 * Whether a row survives the active chips.
 *
 * Chips **union**: they read as "show me these kinds", so any match is enough. With none active
 * every row survives, which is the unfiltered table.
 *
 * @param row - The row to judge.
 * @param chips - The active chips.
 * @param now - The current time.
 * @returns Whether it is shown, hidden, or not yet knowable.
 */
function judge(row: RepoRow, chips: FilterChip[], now: number): Verdict {
  if (chips.length === 0) return 'match';
  // No `RepoStatus` at all: nothing to judge, and the row is the interesting one. See the note at
  // the top of this file.
  if (!isRead(row)) return 'match';

  let waiting = false;
  for (const chip of chips) {
    const verdict = CHIP_RULE[chip](row, now);
    if (verdict === 'match') return 'match';
    if (verdict === 'pending') waiting = true;
  }
  return waiting ? 'pending' : 'no';
}

/**
 * Orders two rows by one column.
 *
 * Direction applies only to the comparison of two real values. A `null` is always last, and two
 * `null`s fall through to the folder-then-name tie-break that keeps the order stable while a scan
 * streams rows in.
 *
 * @param left - One row.
 * @param right - The other.
 * @param key - The column to order by.
 * @param direction - Which way.
 * @returns A comparator result.
 */
export function compareRows(
  left: RepoRow,
  right: RepoRow,
  key: SortKey,
  direction: SortDirection,
): number {
  const first = sortValue(left, key);
  const second = sortValue(right, key);

  if (first === null && second === null) return tieBreak(left, right);
  if (first === null) return 1;
  if (second === null) return -1;

  const base =
    typeof first === 'string' && typeof second === 'string'
      ? first.localeCompare(second)
      : Number(first) - Number(second);
  const signed = direction === 'asc' ? base : -base;

  return signed === 0 ? tieBreak(left, right) : signed;
}

/**
 * What one row compares by under one column.
 *
 * The name is the only column an unread row can answer. Under every other one it sorts last, which
 * is the same claim the table makes about it in every other cell: nothing is known yet.
 *
 * @param row - The row.
 * @param key - The column.
 * @returns Its comparable value, or `null` for no answer.
 */
function sortValue(row: RepoRow, key: SortKey): Sortable {
  if (key === 'name') return row.name;
  return isRead(row) ? SORT_VALUE[key](row) : null;
}

/**
 * Folder, then name — the ordering the store used before there was a sort at all.
 *
 * @param left - One row.
 * @param right - The other.
 * @returns A comparator result.
 */
function tieBreak(left: RepoRow, right: RepoRow): number {
  return left.parent.localeCompare(right.parent) || left.name.localeCompare(right.name);
}

/**
 * Splits sorted rows into one block per containing folder.
 *
 * The blocks are ordered by folder; the rows inside keep the column sort, so grouping narrows the
 * ordering rather than replacing it.
 *
 * @param rows - Rows, already sorted.
 * @returns One group per `parent`, in folder order.
 */
function group(rows: RepoRow[]): RepoGroup[] {
  const byParent = new Map<string, RepoRow[]>();
  for (const row of rows) {
    const bucket = byParent.get(row.parent);
    if (bucket === undefined) byParent.set(row.parent, [row]);
    else bucket.push(row);
  }

  return [...byParent.entries()]
    .map(([key, inGroup]) => ({ key, rows: inGroup }))
    .toSorted((left, right) => left.key.localeCompare(right.key));
}
