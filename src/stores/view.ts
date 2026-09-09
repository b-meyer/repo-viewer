import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import {
  DEFAULT_UI_SETTINGS,
  type FilterChip,
  type SortDirection,
  type SortKey,
  type UiSettings,
} from '@/scripts/settings';

/**
 * How the table is shown: which rows, in what order, grouped or not.
 *
 * A store rather than a module singleton, unlike `scan.ts` and `search.ts`. Everything here is
 * rendered — a chip is a pressed button, the sort is an arrow in a header — so it belongs where the
 * components can reach it reactively.
 *
 * **None of it is row state.** Nothing in this store describes a repository, so nothing in it is
 * mirrored from Rust or sent to Rust as truth. Rust _keeps_ it, which is a different relationship:
 * {@link settings} is the object that gets persisted, and {@link Apply} is what a launch restores.
 *
 * `query` is the exception that is not persisted. A restored query would paint an empty table on
 * launch and make it look as though the repositories were gone.
 */
export const useViewStore = defineStore('view', () => {
  /// Data
  /**
   * The active chips. An array rather than a `Set` because it is persisted as one and iterated far
   * more often than it is tested.
   */
  const chips = ref<FilterChip[]>([...DEFAULT_UI_SETTINGS.chips]);

  /**
   * Which column the rows are ordered by.
   */
  const sortKey = ref<SortKey>(DEFAULT_UI_SETTINGS.sortKey);

  /**
   * Which way that order runs.
   */
  const sortDirection = ref<SortDirection>(DEFAULT_UI_SETTINGS.sortDirection);

  /**
   * Whether rows are grouped under their containing folder.
   */
  const groupByFolder = ref(DEFAULT_UI_SETTINGS.groupByFolder);

  /**
   * What the user has typed into the search box. Not persisted.
   */
  const query = ref('');

  /// Computed
  /**
   * The persisted half of this store, in the shape the settings file holds.
   */
  const settings = computed<UiSettings>(() => ({
    chips: [...chips.value],
    sortKey: sortKey.value,
    sortDirection: sortDirection.value,
    groupByFolder: groupByFolder.value,
  }));

  /**
   * Whether anything is narrowing the table, which is what makes an empty table explicable.
   */
  const filtering = computed(() => chips.value.length > 0 || query.value.trim() !== '');

  /// Methods
  /**
   * Turns one chip on or off.
   *
   * @param chip - The chip that was clicked.
   */
  function ToggleChip(chip: FilterChip): void {
    chips.value = chips.value.includes(chip)
      ? chips.value.filter((active) => active !== chip)
      : [...chips.value, chip];
  }

  /**
   * Sorts by a column, or reverses the sort if it is already the active one.
   *
   * A new column always starts ascending. Carrying the previous column's direction over makes a
   * header click do two things at once, which reads as a bug.
   *
   * @param key - The column that was clicked.
   */
  function SortBy(key: SortKey): void {
    if (sortKey.value === key) {
      sortDirection.value = sortDirection.value === 'asc' ? 'desc' : 'asc';
      return;
    }
    sortKey.value = key;
    sortDirection.value = 'asc';
  }

  /**
   * Turns grouping on or off.
   *
   * @param on - Whether to group.
   */
  function SetGroupByFolder(on: boolean): void {
    groupByFolder.value = on;
  }

  /**
   * Records what the user typed.
   *
   * @param next - The query.
   */
  function SetQuery(next: string): void {
    query.value = next;
  }

  /**
   * Clears every filter, for the escape hatch beside an empty table.
   */
  function ClearFilters(): void {
    chips.value = [];
    query.value = '';
  }

  /**
   * Applies persisted settings wholesale, at launch.
   *
   * @param next - Settings, already parsed and complete.
   */
  function Apply(next: UiSettings): void {
    chips.value = [...next.chips];
    sortKey.value = next.sortKey;
    sortDirection.value = next.sortDirection;
    groupByFolder.value = next.groupByFolder;
  }

  return {
    chips,
    sortKey,
    sortDirection,
    groupByFolder,
    query,
    settings,
    filtering,
    ToggleChip,
    SortBy,
    SetGroupByFolder,
    SetQuery,
    ClearFilters,
    Apply,
  };
});
