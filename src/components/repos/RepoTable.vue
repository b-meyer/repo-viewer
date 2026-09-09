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
          :read-error="readErrors?.get(row.path) ?? null"
          :tier0-done="readErrors !== null"
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
   * Tier 0's failures by path, or `null` while it is still running.
   *
   * `null` is also what tells a row it is still waiting rather than unreadable, so an empty map and
   * a missing one mean genuinely different things here.
   */
  readErrors: Map<string, string> | null;
  /**
   * What to say when there is nothing to show.
   */
  emptyMessage: string;
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
