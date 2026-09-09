<template>
  <div class="flex flex-col gap-8 border-b border-gray-200 px-20 py-10">
    <app-progress
      :value="progress.total === null ? null : progress.read"
      :max="progress.total ?? 1"
      label="Repositories read"
    />

    <div class="text-12 text-gray-600" v-text="line" />

    <scan-errors v-if="discovery" :errors="discovery.errors" title="could not be walked" />
    <!-- Built from the accumulated map rather than from `totals.errors`, so a repository that
         failed shows up while the scan is still running instead of at the end of it. -->
    <scan-errors v-if="readErrors.length > 0" :errors="readErrors" title="could not be read" />
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import ScanErrors from '@/components/feedback/ScanErrors.vue';
import AppProgress from '@/components/inputs/AppProgress.vue';
import type { ScanError } from '@/scripts/generated/ScanError';
import type { ScanSummary } from '@/scripts/generated/ScanSummary';
import type { ScanTotals } from '@/scripts/generated/ScanTotals';
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
   * What the whole scan did, tier by tier, once it has finished.
   */
  totals: ScanTotals | null;
  /**
   * Why each repository produced no row, by path, accumulated as the scan runs.
   */
  repoErrors: Map<string, string>;
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

  const totals = props.totals;
  const parts = [`${String(read)} ${read === 1 ? 'repository' : 'repositories'}`];
  if (props.discovery) parts.push(`${String(props.discovery.dirsPruned)} pruned`);
  if (totals) {
    // Four numbers, and the first is not the sum of the other three: most of a scan is spent
    // waiting for the walk to hand over the next batch, which belongs to no tier. Tier 1 is
    // typically an order of magnitude above Tier 0, which is the whole reason they stream apart —
    // so showing them separately is what makes that visible instead of merely documented.
    parts.push(
      `scan ${String(totals.elapsedMs)} ms`,
      `discovery ${String(totals.discoveryMs)} ms`,
      `tier 0 ${String(totals.tier0Ms)} ms`,
      `tier 1 ${String(totals.tier1Ms)} ms`,
    );
  }
  return parts.join(' · ');
});

/**
 * The read failures as a list, for the errors panel.
 */
const readErrors = computed<ScanError[]>(() =>
  [...props.repoErrors].map(([path, message]) => ({ path, message })),
);
</script>
