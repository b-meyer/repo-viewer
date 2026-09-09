<template>
  <div v-if="rows.length === 0" class="text-14 p-20 text-gray-600" v-text="emptyMessage" />

  <div v-else class="scrollbar min-h-0 flex-1 overflow-auto">
    <table class="w-full border-collapse text-left">
      <thead class="bg-gray-25 sticky top-0 z-1 border-b border-gray-200">
        <tr class="h-28">
          <th
            v-for="column in COLUMNS"
            :key="column.key"
            :class="['text-11 px-10 font-semibold text-gray-600', column.class]"
            v-text="column.label"
          />
        </tr>
      </thead>
      <tbody>
        <repo-row
          v-for="row in rows"
          :key="row.path"
          :row="row"
          :now="now"
          :read-error="repoErrors.get(row.path) ?? null"
          :tier0-done="tier0Done"
          :expanded="expanded.has(row.path)"
          :loading-detail="loadingDetail.has(row.path)"
          :detail-error="detailErrors.get(row.path) ?? null"
          :colspan="COLUMNS.length"
          @toggle="emit('toggle', row.path)"
          @refresh="emit('refresh', row.path)"
        />
      </tbody>
    </table>
  </div>
</template>

<script setup lang="ts">
import RepoRow from '@/components/repos/RepoRow.vue';
import type { RepoRow as Row } from '@/stores/repos';

/// Setup
defineProps<{
  /**
   * The rows to render, already ordered.
   */
  rows: Row[];
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
}>();

/// Data
/**
 * The columns, in order.
 *
 * Upstream and sync is deliberately **one** column. Ahead/behind is measured against
 * `refs/remotes/*` and means nothing without the age of the fetch that populated them, so the two
 * share a cell and a component — there is no markup path that renders the counts alone.
 */
const COLUMNS = [
  { key: 'name', label: 'Repository', class: 'min-w-280' },
  { key: 'head', label: 'Head', class: 'w-200' },
  { key: 'sync', label: 'Upstream & sync', class: 'w-200' },
  { key: 'state', label: 'State', class: 'w-110' },
  { key: 'stash', label: 'Stash', class: 'w-70 text-right' },
  { key: 'worktree', label: 'Worktree', class: 'w-140' },
  { key: 'read', label: 'Read', class: 'w-90 text-right' },
];
</script>
