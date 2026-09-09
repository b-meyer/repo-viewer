/**
 * The view state, and how to read it back from disk safely.
 *
 * **This is the one shape on the wire that `ts-rs` does not generate**, and the exception is
 * deliberate. Chips, sort keys and grouping are vocabulary of the table; Rust stores the object and
 * never looks inside it, so typing it in Rust would mean either putting UI concepts in the engine
 * crate — the crate that gets swapped for a different domain — or a `ts-rs` derive in `src-tauri`
 * that breaks `vp run types`' scoping. `CommandError` crossing as a bare string is the same
 * exception for the same kind of reason.
 *
 * The cost is that what comes back is genuinely untyped: the file is hand-editable and nothing
 * validates it on the way in. So {@link parseUiSettings} treats every field as absent until proven
 * otherwise, and a garbage file degrades to the defaults rather than to a broken table.
 */

/**
 * A filter chip. Each names a state a user goes looking for.
 */
export type FilterChip = 'dirty' | 'unpushed' | 'detached' | 'conflicted' | 'staleFetch';

/**
 * What a column sorts by. One per sortable column in `RepoTable`, so a header click maps to exactly
 * one of these.
 */
export type SortKey = 'name' | 'head' | 'sync' | 'state' | 'stash' | 'worktree' | 'read';

/**
 * Which way a sort runs. Unknown values sort last in **both** directions — see `view.ts`.
 */
export type SortDirection = 'asc' | 'desc';

/**
 * Everything about how the table is currently shown, and everything that is persisted.
 *
 * The search query is **not** here on purpose. Restoring one would paint an empty table on launch
 * and blame the repositories for it.
 */
export type UiSettings = {
  /**
   * The active chips. Empty means no filtering.
   */
  chips: FilterChip[];
  /**
   * Which column the rows are ordered by.
   */
  sortKey: SortKey;
  /**
   * Which way that order runs.
   */
  sortDirection: SortDirection;
  /**
   * Whether rows are grouped under their containing folder.
   */
  groupByFolder: boolean;
};

/**
 * Every chip, in the order they are shown.
 */
export const FILTER_CHIPS: readonly FilterChip[] = [
  'dirty',
  'unpushed',
  'detached',
  'conflicted',
  'staleFetch',
];

/**
 * Every sortable column key.
 */
export const SORT_KEYS: readonly SortKey[] = [
  'name',
  'head',
  'sync',
  'state',
  'stash',
  'worktree',
  'read',
];

/**
 * What the table looks like before anyone has changed anything.
 *
 * Ordering by name ascending, which is what the store's own fallback ordering did before there was
 * a sort at all — so a first run looks the same as it always has.
 */
export const DEFAULT_UI_SETTINGS: UiSettings = {
  chips: [],
  sortKey: 'name',
  sortDirection: 'asc',
  groupByFolder: false,
};

/**
 * A fresh copy of the defaults.
 *
 * Spreading {@link DEFAULT_UI_SETTINGS} is **not** equivalent: the spread is shallow, so every
 * caller would share one `chips` array and the first chip a user clicked would edit what the next
 * fallback returns. A test pins this.
 *
 * @returns Defaults nothing else holds a reference into.
 */
export function defaultUiSettings(): UiSettings {
  return { ...DEFAULT_UI_SETTINGS, chips: [...DEFAULT_UI_SETTINGS.chips] };
}

/**
 * Reads persisted view state, falling back to the default for anything it cannot trust.
 *
 * Field by field rather than all-or-nothing: a file with a good sort and a nonsense chip should
 * keep the sort. There is no schema validator in this project and this is the only value that
 * arrives unvalidated, so the narrowing is written out by hand.
 *
 * @param value - Whatever the settings file held, which may be anything at all.
 * @returns A complete, valid `UiSettings`.
 */
export function parseUiSettings(value: unknown): UiSettings {
  if (typeof value !== 'object' || value === null) return defaultUiSettings();

  // `in` is what makes these reads type-check with no assertion anywhere: it narrows an `object`
  // to one known to carry the key, and the value then reads back as `unknown` — which is what it
  // genuinely is. Asserting a `Record<string, unknown>` instead would claim a shape nothing has
  // verified, and that is the one thing this file exists to avoid. Four literals rather than a
  // helper, because a helper would take the key as a runtime string and be back where it started.
  const chips = 'chips' in value ? value.chips : undefined;
  const sortKey = 'sortKey' in value ? value.sortKey : undefined;
  const sortDirection = 'sortDirection' in value ? value.sortDirection : undefined;
  const groupByFolder = 'groupByFolder' in value ? value.groupByFolder : undefined;

  return {
    chips: Array.isArray(chips) ? chips.filter(isChip) : defaultUiSettings().chips,
    sortKey: isSortKey(sortKey) ? sortKey : DEFAULT_UI_SETTINGS.sortKey,
    sortDirection: isSortDirection(sortDirection)
      ? sortDirection
      : DEFAULT_UI_SETTINGS.sortDirection,
    groupByFolder:
      typeof groupByFolder === 'boolean' ? groupByFolder : DEFAULT_UI_SETTINGS.groupByFolder,
  };
}

/**
 * Whether `value` is one of the known chips.
 *
 * @param value - A candidate from the settings file.
 * @returns Whether it can be used.
 */
function isChip(value: unknown): value is FilterChip {
  return typeof value === 'string' && FILTER_CHIPS.some((chip) => chip === value);
}

/**
 * Whether `value` is one of the sortable columns.
 *
 * @param value - A candidate from the settings file.
 * @returns Whether it can be used.
 */
function isSortKey(value: unknown): value is SortKey {
  return typeof value === 'string' && SORT_KEYS.some((key) => key === value);
}

/**
 * Whether `value` is a sort direction.
 *
 * @param value - A candidate from the settings file.
 * @returns Whether it can be used.
 */
function isSortDirection(value: unknown): value is SortDirection {
  return value === 'asc' || value === 'desc';
}
