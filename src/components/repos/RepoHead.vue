<template>
  <span v-if="head.kind === 'branch'" class="flex items-center gap-6">
    <i class="bi bi-git text-gray-500" />
    <span class="text-12 font-mono" v-text="head.name" />
  </span>

  <span
    v-else-if="head.kind === 'detached'"
    class="flex items-center gap-6"
    :title="`Detached HEAD at ${head.id}`"
  >
    <i class="bi bi-bezier2 text-orange-600" />
    <span class="text-12 font-mono text-orange-700" v-text="shortId(head.id)" />
  </span>

  <span v-else class="text-12 text-gray-500 italic" title="No commits yet" v-text="'unborn'" />
</template>

<script setup lang="ts">
import type { Head } from '@/scripts/generated/Head';
import { shortId } from '@/scripts/utils';

/// Setup
defineProps<{
  /**
   * Where HEAD points.
   *
   * A discriminated union from the Rust model, so the template narrows on `kind` rather than
   * testing three optional fields against each other.
   */
  head: Head;
}>();
</script>
