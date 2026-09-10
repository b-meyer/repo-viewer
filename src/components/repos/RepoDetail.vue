<template>
  <div class="bg-gray-25 border-gray-150 flex flex-col gap-12 border-b px-10 py-12">
    <!-- Counts -->
    <div class="flex flex-col gap-6">
      <div class="flex items-baseline gap-8">
        <span class="text-11 font-semibold text-gray-600" v-text="'Changes'" />
        <!-- Four column totals, not four slices of one set: a staged-then-modified file is `MM` to
             git and appears in two of them. So there is no total here, and there must not be. -->
        <span class="text-11 text-gray-450" v-text="'by git status column'" />
      </div>

      <app-unknown v-if="absenceFor(counts)" :reason="absenceFor(counts)!" :hint="absenceHint" />
      <dl v-else-if="counts" class="flex flex-wrap gap-x-24 gap-y-4">
        <div v-for="column in COLUMNS" :key="column.key" class="flex items-baseline gap-6">
          <dt class="text-11 text-gray-600" v-text="column.label" />
          <dd
            :class="['text-13 font-medium', counts[column.key] > 0 ? column.tone : 'text-gray-450']"
            v-text="counts[column.key]"
          />
        </div>
      </dl>
    </div>

    <!-- Submodules -->
    <div class="flex flex-col gap-6">
      <span class="text-11 font-semibold text-gray-600" v-text="'Submodules'" />

      <app-unknown
        v-if="absenceFor(submodules)"
        :reason="absenceFor(submodules)!"
        :hint="absenceHint"
      />
      <!-- Read, and there are none. An em dash rather than `0`: the question was answered. -->
      <app-unknown v-else-if="submodules && submodules.length === 0" reason="none" />
      <ul v-else-if="submodules" class="flex flex-col gap-4">
        <li v-for="module in submodules" :key="module.path" class="flex items-baseline gap-8">
          <span class="text-12 font-medium" v-text="module.name" />
          <span class="text-11 font-mono text-gray-500" v-text="module.path" />
          <!-- A submodule that is not checked out has no HEAD of its own, which is a real answer
               and not a failure. -->
          <app-unknown
            v-if="module.headId === null"
            reason="na"
            hint="Not checked out — this submodule has no HEAD of its own."
          />
          <span
            v-else-if="module.recordedId !== null && module.headId !== module.recordedId"
            class="text-11 text-orange-600"
            :title="`Parent records ${shortId(module.recordedId)}, submodule is at ${shortId(module.headId)}.`"
            v-text="'moved'"
          />
          <span v-else class="text-11 text-green-600" v-text="'in sync'" />
        </li>
      </ul>
    </div>

    <!-- A read that failed while values from an earlier one are still on the row: the numbers are
         real, so they stay, and the failure is reported beside them rather than replacing them. -->
    <app-alert v-if="detailError !== null && counts !== null" tone="warning" title="Re-read failed">
      {{ detailError }}
    </app-alert>

    <!-- Actions -->
    <repo-actions
      :open-error="openError"
      :fetch-state="fetchState"
      :fetch-error="fetchError"
      :git-missing="gitMissing"
      @open="emit('open', $event)"
      @fetch="emit('fetch')"
    >
      <app-button
        label="Re-read"
        variant="outline"
        size="small"
        icon="bi-arrow-clockwise"
        :busy="loading"
        @click="emit('refresh')"
      />
      <span class="text-11 text-gray-500" v-text="`Read ${formatAge(row.scannedAtMs, now)}`" />
    </repo-actions>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import AppAlert from '@/components/feedback/AppAlert.vue';
import AppUnknown, { type UnknownReason } from '@/components/feedback/AppUnknown.vue';
import AppButton from '@/components/inputs/AppButton.vue';
import RepoActions from '@/components/repos/RepoActions.vue';
import type { FileCounts } from '@/scripts/generated/FileCounts';
import type { RepoStatus } from '@/scripts/generated/RepoStatus';
import type { OpenTarget } from '@/scripts/ipc';
import { formatAge, shortId } from '@/scripts/utils';
import type { FetchRowState } from '@/stores/repos';

/// Setup
const props = defineProps<{
  /**
   * The row this drawer belongs to. Always one Tier 0 has read — a row without a status has no
   * counts to show and no command that would fetch them.
   */
  row: RepoStatus;
  /**
   * Whether a Tier 2 read is in flight for this row.
   */
  loading: boolean;
  /**
   * Why the last Tier 2 read failed, if it did.
   */
  detailError: string | null;
  /**
   * Why the last open-in launch failed, if it did.
   */
  openError: string | null;
  /**
   * Whether a fetch of this row is queued, running, or neither.
   */
  fetchState: FetchRowState | null;
  /**
   * Why the last fetch of this row failed, if it did.
   */
  fetchError: string | null;
  /**
   * Why fetching is unavailable, or `null` when it is available.
   */
  gitMissing: string | null;
  /**
   * The current time, for the age line.
   */
  now: number;
}>();

const emit = defineEmits<{
  /**
   * The user asked for a fresh read.
   */
  refresh: [];
  /**
   * The user asked to open this repository elsewhere.
   */
  open: [target: OpenTarget];
  /**
   * The user asked to fetch this repository.
   */
  fetch: [];
}>();

/// Computed
/**
 * The counts, or `null` when there are none to show.
 */
const counts = computed<FileCounts | null>(() => props.row.counts);

/**
 * The submodule list, or `null` when it has not been read.
 */
const submodules = computed(() => props.row.submodules);

/**
 * Which kind of absence stands in for a missing value, or `null` when there is a value to show.
 *
 * **Applied per field, not once for the drawer.** Tier 2's two halves are read independently and
 * either can fail alone — the status iterator touches the worktree, the submodule list reads
 * `.gitmodules` and possibly the whole index — so a single answer for both would leave one section
 * rendering nothing at all whenever the other succeeded.
 *
 * The order of the checks is the load-bearing part, because three of the four reasons can be true
 * at once:
 *
 * - `na` first — a bare repository has no worktree to diff and no `.gitmodules` to read, so neither
 *   can _ever_ be computed. A failed read of something unreadable by nature is still `n/a`.
 * - `unreadable` next — the read was tried and failed. Ahead of `pending`, because a retry being in
 *   flight does not make the previous failure untrue.
 * - `pending` last, and it covers both "a read is running" and "the drawer just opened and the read
 *   is about to start". Both are genuinely transient, which is what that word claims.
 *
 * @param value - The field to describe.
 * @returns Which absence to render, or `null` to render the value.
 */
function absenceFor(value: unknown): UnknownReason | null {
  if (props.row.kind === 'bare') return 'na';
  if (value !== null) return null;
  if (props.detailError !== null) return 'unreadable';
  return 'pending';
}

/**
 * The hover text for whichever absence is showing.
 */
const absenceHint = computed(() => {
  if (props.row.kind === 'bare') return 'Bare repository — no worktree and no .gitmodules to read.';
  return props.detailError;
});

/// Data
/**
 * The four columns, in `git status` order: index side first, then worktree, then the two that
 * belong to neither.
 */
const COLUMNS = [
  { key: 'staged', label: 'Staged', tone: 'text-green-700' },
  { key: 'unstaged', label: 'Unstaged', tone: 'text-orange-600' },
  { key: 'untracked', label: 'Untracked', tone: 'text-gray-700' },
  { key: 'conflicted', label: 'Conflicted', tone: 'text-red-600' },
] as const satisfies readonly { key: keyof FileCounts; label: string; tone: string }[];
</script>
