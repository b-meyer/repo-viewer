<template>
  <span
    v-if="state !== 'clean'"
    :class="['text-11 inline-flex h-18 items-center rounded border px-6', TONE[state]]"
    v-text="LABEL[state]"
  />
</template>

<script setup lang="ts">
import type { RepoState } from '@/scripts/generated/RepoState';

/// Setup
defineProps<{
  /**
   * The in-progress operation, if any.
   */
  state: RepoState;
}>();

/// Data
/**
 * How each parked operation reads.
 *
 * `clean` has no entry and renders nothing at all: an empty cell is the quiet default, and a badge
 * saying "clean" on every row would drown the five that matter.
 */
const LABEL: Record<RepoState, string> = {
  clean: '',
  merging: 'merging',
  rebasing: 'rebasing',
  bisecting: 'bisecting',
  cherryPicking: 'cherry-pick',
  reverting: 'reverting',
};

/**
 * Orange for an interrupted write, purple for a bisect, which is a search rather than a change.
 */
const TONE: Record<RepoState, string> = {
  clean: '',
  merging: 'border-orange-300 bg-orange-50 text-orange-700',
  rebasing: 'border-orange-300 bg-orange-50 text-orange-700',
  bisecting: 'border-purple-300 bg-purple-50 text-purple-700',
  cherryPicking: 'border-orange-300 bg-orange-50 text-orange-700',
  reverting: 'border-orange-300 bg-orange-50 text-orange-700',
};
</script>
