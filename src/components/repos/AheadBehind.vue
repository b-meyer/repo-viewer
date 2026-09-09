<template>
  <div class="flex flex-col gap-2">
    <!-- No upstream: there is nothing to be ahead of, and nothing to be stale about. -->
    <span
      v-if="upstream === null"
      class="text-11 text-gray-500"
      title="No upstream branch is configured."
      v-text="'no upstream'"
    />

    <template v-else>
      <span class="text-11 font-mono text-gray-600" v-text="upstream" />

      <!-- Configured, but no remote-tracking ref to count against: never fetched, or the remote
           branch was deleted. A distinct state from "in sync", and showing 0/0 here would invent
           an answer. -->
      <span
        v-if="ahead === null && behind === null"
        class="text-11 text-orange-600"
        title="An upstream is configured but no remote-tracking ref exists — never fetched, or the remote branch was deleted."
        v-text="'not tracked'"
      />

      <span
        v-else-if="ahead === 0 && behind === 0"
        class="text-12 text-gray-500"
        v-text="'in sync'"
      />

      <span v-else class="text-12 flex items-center gap-8">
        <span
          v-if="ahead !== null && ahead > 0"
          class="text-green-600"
          :aria-label="`${formatCount(ahead)} ahead`"
          :title="
            atCap(ahead) ? 'At least this many — the revision walk stops at the cap.' : undefined
          "
          v-text="`↑${formatCount(ahead)}`"
        />
        <span
          v-if="behind !== null && behind > 0"
          class="text-orange-600"
          :aria-label="`${formatCount(behind)} behind`"
          :title="
            atCap(behind) ? 'At least this many — the revision walk stops at the cap.' : undefined
          "
          v-text="`↓${formatCount(behind)}`"
        />
      </span>

      <!-- Always rendered beside the counts, never conditionally. -->
      <span
        :class="['text-11', FETCH_TONE[staleness]]"
        v-text="formatFetchAge(lastFetchedMs, now)"
      />
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { AHEAD_BEHIND_CAP, fetchStaleness, formatCount, formatFetchAge } from '@/scripts/utils';

/// Setup
const props = defineProps<{
  /**
   * Commits ahead of upstream, or `null` when there is nothing to count against.
   */
  ahead: number | null;
  /**
   * Commits behind upstream, or `null`.
   */
  behind: number | null;
  /**
   * The upstream ref's short name, or `null` when none is configured.
   */
  upstream: string | null;
  /**
   * When `FETCH_HEAD` was last written, or `null` if never.
   */
  lastFetchedMs: number | null;
  /**
   * The current time, passed in so rendering is pure.
   */
  now: number;
}>();

/// Computed
/**
 * Which staleness band the last fetch falls in.
 */
const staleness = computed(() => fetchStaleness(props.lastFetchedMs, props.now));

/// Data
/**
 * How loudly the fetch age is shown.
 *
 * Never-fetched and long-stale are orange and red because the counts above them are measured
 * against `refs/remotes/*` and are only as fresh as this. Counts presented without that
 * qualification are the single most misleading thing this app could show, which is why the age
 * lives in the same component as the numbers rather than in a column of its own — there is no
 * markup path that renders one without the other.
 */
const FETCH_TONE: Record<string, string> = {
  never: 'text-orange-600',
  fresh: 'text-gray-500',
  stale: 'text-orange-600',
  'very-stale': 'text-red-600',
};

/// Methods
/**
 * Whether a count hit the revision-walk cap and therefore means "at least this many".
 *
 * @param count - The count to test.
 * @returns Whether it is capped.
 */
function atCap(count: number): boolean {
  return count >= AHEAD_BEHIND_CAP;
}
</script>
