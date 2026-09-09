<template>
  <details v-if="errors.length > 0" class="text-12">
    <summary class="cursor-pointer text-orange-700">
      {{ errors.length }} {{ errors.length === 1 ? 'path' : 'paths' }} {{ title }}
    </summary>
    <ul class="mt-6 flex flex-col gap-4">
      <li v-for="error in errors" :key="error.path" class="text-11">
        <span class="font-mono text-gray-700" v-text="error.path" />
        <span class="text-gray-600" v-text="` — ${error.message}`" />
      </li>
    </ul>
  </details>
</template>

<script setup lang="ts">
import type { ScanError } from '@/scripts/generated/ScanError';

/// Setup
defineProps<{
  /**
   * The failures to list.
   *
   * The walk's failures and Tier 0's are the same type shown the same way. Tier 0's are the
   * repositories that produced no row at all — the most important thing on the screen when the list
   * is not empty, and the reason they are delivered per batch rather than only at the end.
   */
  errors: ScanError[];
  /**
   * What went wrong with them, e.g. `could not be read`.
   */
  title: string;
}>();
</script>
