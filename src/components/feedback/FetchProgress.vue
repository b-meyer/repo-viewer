<template>
  <div class="flex flex-col gap-8 border-b border-gray-200 px-20 py-10">
    <app-progress :value="progress.settled" :max="progress.total" label="Repositories fetched" />

    <div class="text-12 text-gray-600" v-text="line" />

    <!-- The same panel the scan uses, because a fetch failure has the same shape: a path and a
         message. A second errors component would be a second place to get the wording right. -->
    <scan-errors v-if="errors.length > 0" :errors="errors" title="could not be fetched" />
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import ScanErrors from '@/components/feedback/ScanErrors.vue';
import AppProgress from '@/components/inputs/AppProgress.vue';
import type { ScanError } from '@/scripts/generated/ScanError';
import type { FetchProgress } from '@/stores/repos';

/**
 * How a fetch is going.
 *
 * **Determinate, unlike `ScanProgress` — and a reader copying that component will get this
 * backwards.** A scan's bar is indeterminate while discovery runs because the denominator does not
 * exist yet; a fetch is handed its exact list before the first process starts, so claiming
 * ignorance of it would be the uncomputed-is-not-zero rule pointed the other way.
 *
 * The bar cannot carry the honest part, which is why the line beneath it exists: 299 warm local
 * repositories finish in a second and one VPN'd monorepo takes a minute, so the bar sits at 99% for
 * most of the wall clock. The line says how many are still running, and how many failed.
 */

/// Setup
const props = defineProps<{
  /**
   * Where the fetch has got to.
   */
  progress: FetchProgress;
  /**
   * Why each row's fetch failed, by path.
   */
  fetchErrors: Map<string, string>;
}>();

/// Computed
/**
 * The status line under the bar.
 */
const line = computed(() => {
  const { total, settled, running, failed } = props.progress;

  const parts = [`Fetched ${String(settled)} of ${String(total)}`];
  if (running > 0) parts.push(`${String(running)} running`);
  if (failed > 0) parts.push(`${String(failed)} failed`);
  return parts.join(' · ');
});

/**
 * The failures as a list, for the errors panel.
 */
const errors = computed<ScanError[]>(() =>
  [...props.fetchErrors].map(([path, message]) => ({ path, message })),
);
</script>
