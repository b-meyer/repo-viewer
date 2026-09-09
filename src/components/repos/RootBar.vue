<template>
  <div class="bg-gray-25 flex flex-wrap items-center gap-10 border-b border-gray-200 px-20 py-10">
    <app-button
      label="Add folder…"
      icon="bi-folder-plus"
      variant="primary"
      size="small"
      :disabled="disabled"
      @click="emit('pick')"
    />

    <app-button
      v-if="scanning"
      label="Cancel"
      icon="bi-x-circle"
      variant="outline"
      size="small"
      @click="emit('cancel')"
    />
    <app-button
      v-else
      label="Scan"
      icon="bi-search"
      size="small"
      :disabled="disabled || roots.length === 0"
      @click="emit('scan')"
    />

    <div class="flex flex-wrap items-center gap-8">
      <span
        v-for="root in roots"
        :key="root"
        class="text-11 flex items-center gap-6 rounded border border-gray-300 bg-white py-2 pr-2 pl-8 text-gray-700"
      >
        <span class="font-mono" v-text="root" />
        <button
          type="button"
          class="cursor-pointer text-gray-500 hover:text-red-600"
          :title="`Stop watching ${root}`"
          :aria-label="`Remove ${root}`"
          @click="emit('remove', root)"
        >
          <i class="bi bi-x" />
        </button>
      </span>
    </div>
  </div>
</template>

<script setup lang="ts">
import AppButton from '@/components/inputs/AppButton.vue';

/// Setup
defineProps<{
  /**
   * The configured roots, as Rust spells them.
   */
  roots: string[];
  /**
   * Whether a scan is in flight, which swaps Scan for Cancel.
   */
  scanning: boolean;
  /**
   * Whether the backend is unreachable, which disables everything that would call it.
   */
  disabled: boolean;
}>();

const emit = defineEmits<{
  /**
   * Open the folder picker.
   */
  pick: [];
  /**
   * Start a scan over every configured root.
   */
  scan: [];
  /**
   * Stop the running scan.
   */
  cancel: [];
  /**
   * Remove one root, by path.
   */
  remove: [root: string];
}>();
</script>
