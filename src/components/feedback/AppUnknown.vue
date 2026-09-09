<template>
  <span :class="TONE[reason]" :title="hint ?? undefined" v-text="LABEL[reason]" />
</template>

<script lang="ts">
/**
 * Why there is no value to show.
 *
 * Four reasons rather than one, because collapsing them is how the rule gets diluted: "not counted
 * yet", "there is genuinely none", "this can never apply here", and "we tried and failed" are four
 * different facts, and rendering them identically throws away the one the reader needs.
 */
export type UnknownReason = 'pending' | 'none' | 'na' | 'unreadable';
</script>

<script setup lang="ts">
/// Setup
defineProps<{
  /**
   * Which kind of absence this is.
   */
  reason: UnknownReason;
  /**
   * Hover text — the cause, for `unreadable`, or why not, for `none` and `na`.
   */
  hint?: string | null;
}>();

/// Data
/**
 * What each reason reads as.
 *
 * `counting…` is verbatim rather than paraphrased: it says work is outstanding, where a dash would
 * say the answer is nothing. This component is the single place either word is written, so the
 * distinction cannot drift between columns.
 */
const LABEL: Record<UnknownReason, string> = {
  pending: 'counting…',
  none: '—',
  na: 'n/a',
  unreadable: 'unreadable',
};

/**
 * How each reason is coloured. Only a genuine failure is loud.
 */
const TONE: Record<UnknownReason, string> = {
  pending: 'text-11 text-gray-500 italic',
  none: 'text-12 text-gray-450',
  na: 'text-11 text-gray-450',
  unreadable: 'text-11 text-red-600',
};
</script>
