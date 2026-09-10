<template>
  <div v-if="empty" class="text-14 p-20 text-gray-600" v-text="emptyMessage" />

  <div v-else class="scrollbar min-h-0 flex-1 overflow-auto">
    <table class="w-full border-collapse text-left">
      <thead class="bg-gray-25 sticky top-0 z-1 border-b border-gray-200">
        <tr class="h-28">
          <th
            v-for="column in COLUMNS"
            :key="column.key"
            :class="['text-11 px-10 font-semibold text-gray-600', column.class]"
            :aria-sort="ariaSort(column.key)"
          >
            <!-- A real button, so the header is reachable by keyboard and announces itself as the
                 control it is. `aria-sort` sits on the cell, which is where it belongs. -->
            <button
              type="button"
              class="flex w-full cursor-pointer items-center gap-4 text-left hover:text-gray-900"
              :class="column.class?.includes('text-right') ? 'justify-end' : ''"
              @click="emit('sort', column.key)"
            >
              <span v-text="column.label" />
              <i v-if="sortKey === column.key" :class="`bi ${arrow} text-10`" />
            </button>
          </th>
        </tr>
      </thead>

      <tbody v-for="group in groups" :key="group.key || 'all'">
        <!-- Only when grouping is on, which is the only time a group has a key. -->
        <tr v-if="group.key !== ''" class="bg-gray-75 border-y border-gray-200">
          <th
            :colspan="COLUMNS.length"
            class="text-11 px-10 py-4 font-mono font-semibold text-gray-700"
            scope="colgroup"
          >
            {{ group.key }}
            <span class="ml-6 font-sans text-gray-500" v-text="`${group.rows.length}`" />
          </th>
        </tr>

        <repo-row
          v-for="row in group.rows"
          :key="row.path"
          :row="row"
          :now="now"
          :read-error="repoErrors.get(row.path) ?? null"
          :tier0-done="tier0Done"
          :expanded="expanded.has(row.path)"
          :loading-detail="loadingDetail.has(row.path)"
          :detail-error="detailErrors.get(row.path) ?? null"
          :open-error="openErrors.get(row.path) ?? null"
          :fetch-state="fetchStates.get(row.path) ?? null"
          :fetch-error="fetchErrors.get(row.path) ?? null"
          :git-missing="gitMissing"
          :colspan="COLUMNS.length"
          @toggle="emit('toggle', row.path)"
          @refresh="emit('refresh', row.path)"
          @open="emit('open', row.path, $event)"
          @fetch="emit('fetch', row.path)"
        />
      </tbody>
    </table>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import RepoRow from '@/components/repos/RepoRow.vue';
import type { OpenTarget } from '@/scripts/ipc';
import type { SortDirection, SortKey } from '@/scripts/settings';
import type { RepoGroup } from '@/scripts/view';
import type { FetchRowState } from '@/stores/repos';

/// Setup
const props = defineProps<{
  /**
   * The rows to render, already filtered, sorted and grouped.
   *
   * One group with an empty key when grouping is off, so the template has one shape rather than
   * two.
   */
  groups: RepoGroup[];
  /**
   * Which column the rows are ordered by.
   */
  sortKey: SortKey;
  /**
   * Which way that order runs.
   */
  sortDirection: SortDirection;
  /**
   * The current time, for the age columns.
   */
  now: number;
  /**
   * Why each repository produced no row, by path.
   *
   * Always a map. These arrive per batch during the scan rather than only at the end, so an entry
   * here is enough on its own to say a row is unreadable — `tier0Done` is the _separate_ question
   * of whether a row with no entry and no status is still waiting.
   */
  repoErrors: Map<string, string>;
  /**
   * Whether Tier 0 has finished, which is what turns "not yet" into "never".
   */
  tier0Done: boolean;
  /**
   * Which rows have their detail drawer open, by path.
   */
  expanded: Set<string>;
  /**
   * Which rows have a Tier 2 read in flight, by path.
   */
  loadingDetail: Set<string>;
  /**
   * Why each row's last Tier 2 read failed, by path.
   */
  detailErrors: Map<string, string>;
  /**
   * Why each row's last open-in launch failed, by path.
   */
  openErrors: Map<string, string>;
  /**
   * Which rows have a fetch queued or running, by path.
   */
  fetchStates: Map<string, FetchRowState>;
  /**
   * Why each row's last fetch failed, by path.
   */
  fetchErrors: Map<string, string>;
  /**
   * Why fetching is unavailable, or `null` when it is available.
   */
  gitMissing: string | null;
  /**
   * What to say when there is nothing to show.
   */
  emptyMessage: string;
}>();

const emit = defineEmits<{
  /**
   * A row's expander was clicked.
   */
  toggle: [path: string];
  /**
   * A row's drawer asked for a fresh read.
   */
  refresh: [path: string];
  /**
   * A row asked to be opened in an external tool.
   */
  open: [path: string, target: OpenTarget];
  /**
   * A row asked to be fetched.
   */
  fetch: [path: string];
  /**
   * A column header was clicked.
   */
  sort: [key: SortKey];
}>();

/// Data
/**
 * The columns, in order.
 *
 * Upstream and sync is deliberately **one** column. Ahead/behind is measured against
 * `refs/remotes/*` and means nothing without the age of the fetch that populated them, so the two
 * share a cell and a component — there is no markup path that renders the counts alone.
 *
 * Every `key` here is a `SortKey`, which is what makes a header click resolve to exactly one
 * comparator in `view.ts` rather than to a mapping kept somewhere between them.
 */
const COLUMNS: { key: SortKey; label: string; class: string }[] = [
  { key: 'name', label: 'Repository', class: 'min-w-280' },
  { key: 'head', label: 'Head', class: 'w-200' },
  { key: 'sync', label: 'Upstream & sync', class: 'w-200' },
  { key: 'state', label: 'State', class: 'w-110' },
  { key: 'stash', label: 'Stash', class: 'w-70 text-right' },
  { key: 'worktree', label: 'Worktree', class: 'w-140' },
  { key: 'read', label: 'Read', class: 'w-90 text-right' },
];

/// Computed
/**
 * Whether there is nothing at all to draw.
 *
 * Across every group, not per group: with grouping on, an empty view is a table of no groups, and
 * with it off it is one group of no rows.
 */
const empty = computed(() => props.groups.every((group) => group.rows.length === 0));

/**
 * The arrow for the active column.
 */
const arrow = computed(() =>
  props.sortDirection === 'asc' ? 'bi-caret-up-fill' : 'bi-caret-down-fill',
);

/// Methods
/**
 * What a header cell announces about the current sort.
 *
 * @param key - The column.
 * @returns An `aria-sort` value.
 */
function ariaSort(key: SortKey): 'ascending' | 'descending' | 'none' {
  if (props.sortKey !== key) return 'none';
  return props.sortDirection === 'asc' ? 'ascending' : 'descending';
}
</script>
