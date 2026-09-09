<template>
  <div class="flex flex-col gap-8 border-b border-gray-200 px-20 py-10">
    <app-progress
      :value="progress.total === null ? null : progress.read"
      :max="progress.total ?? 1"
      label="Repositories read"
    />

    <div class="text-12 text-gray-600" v-text="line" />

    <scan-errors v-if="discovery" :errors="discovery.errors" title="could not be walked" />
    <scan-errors v-if="tier0" :errors="tier0.errors" title="could not be read" />
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import ScanErrors from '@/components/feedback/ScanErrors.vue';
import AppProgress from '@/components/inputs/AppProgress.vue';
import type { ScanSummary } from '@/scripts/generated/ScanSummary';
import type { Tier0Summary } from '@/scripts/generated/Tier0Summary';
import type { ScanProgress } from '@/stores/repos';

/// Setup
const props = defineProps<{
  /**
   * Where the scan has got to.
   */
  progress: ScanProgress;
  /**
   * What the walk did, once it has finished.
   */
  discovery: ScanSummary | null;
  /**
   * What Tier 0 did, once the scan has finished.
   */
  tier0: Tier0Summary | null;
}>();

/// Computed
/**
 * The status line under the bar.
 *
 * While discovery is running there is deliberately **no denominator**: the walk has not finished,
 * so the only honest thing to say is how many have turned up so far. A "40 of 128" that keeps
 * revising its own total downward reads as a bug even when it is not.
 */
const line = computed(() => {
  const { phase, found, read, total } = props.progress;

  if (phase === 'discovering') return `Discovering… ${String(found)} found`;
  if (phase === 'reading') {
    return total === null
      ? `Read ${String(read)} of ${String(found)} found so far`
      : `Read ${String(read)} of ${String(total)}`;
  }
  if (phase === 'cancelled') return `Cancelled after ${String(read)} of ${String(found)}`;
  if (phase === 'failed') return 'The scan failed.';

  const walk = props.discovery;
  const reads = props.tier0;
  const parts = [`${String(read)} ${read === 1 ? 'repository' : 'repositories'}`];
  if (walk)
    parts.push(`discovery ${String(walk.elapsedMs)} ms`, `${String(walk.dirsPruned)} pruned`);
  // `scan`, not `Tier 0`: the pipeline puts its whole wall-clock time in this field, walk and
  // Tier 1 included. Labelling it Tier 0 blamed the cheap tier for the expensive one's cost.
  if (reads) parts.push(`scan ${String(reads.elapsedMs)} ms`);
  return parts.join(' · ');
});
</script>
