<template>
  <progress-root
    v-slot="{ modelValue }"
    :model-value="value"
    :max="max"
    :get-value-label="ValueLabel"
    class="relative h-6 w-full overflow-hidden rounded bg-gray-200"
  >
    <!-- reka-ui hands the slot `number | undefined`, and both mean "no determinate value": an
         indeterminate bar, never a zero-width one. -->
    <progress-indicator
      :class="[
        'bg-primary-400 h-full transition-all',
        typeof modelValue !== 'number' && 'w-1/3 animate-pulse',
      ]"
      :style="
        typeof modelValue === 'number' ? { width: `${(modelValue / max) * 100}%` } : undefined
      "
    />
  </progress-root>
</template>

<script setup lang="ts">
import { ProgressIndicator, ProgressRoot } from 'reka-ui';

/// Setup
const props = defineProps<{
  /**
   * How far along, or `null` when the total is not yet known.
   *
   * `null` renders an indeterminate bar. It must never fall back to `0`, which would render an
   * empty determinate bar — "nothing done yet" where the truth is "we do not know how much there
   * is". The unknown-is-not-zero rule, in the one place a progress bar can get it wrong.
   */
  value: number | null;
  /**
   * The denominator.
   */
  max: number;
  /**
   * What is being counted, for the accessible label.
   */
  label: string;
}>();

/// Methods
/**
 * The accessible progress label.
 *
 * @returns A description of the current progress.
 */
function ValueLabel(): string {
  return props.value === null
    ? `${props.label}: counting…`
    : `${props.label}: ${String(props.value)} of ${String(props.max)}`;
}
</script>
