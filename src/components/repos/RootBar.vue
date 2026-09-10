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

    <app-button
      v-if="fetching"
      label="Stop fetching"
      icon="bi-x-circle"
      variant="outline"
      size="small"
      @click="emit('cancelFetch')"
    />
    <app-button
      v-else
      :label="fetchLabel"
      icon="bi-cloud-download"
      :title="fetchTitle"
      variant="outline"
      size="small"
      :disabled="disabled || gitMissing !== null || fetchable === 0"
      @click="emit('fetch')"
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
import { computed } from 'vue';
import AppButton from '@/components/inputs/AppButton.vue';

/// Setup
const props = defineProps<{
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
  /**
   * How many rows the table is currently showing, which is what a fetch would fetch.
   */
  fetchable: number;
  /**
   * How many rows exist at all, so the label can tell "all" from "shown".
   */
  total: number;
  /**
   * Whether a fetch is in flight, which swaps Fetch for Stop.
   */
  fetching: boolean;
  /**
   * Why fetching is unavailable, or `null` when it is available.
   */
  gitMissing: string | null;
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
  /**
   * Fetch every row the table is showing.
   */
  fetch: [];
  /**
   * Stop the running fetch.
   */
  cancelFetch: [];
}>();

/// Computed
/**
 * "All" only when nothing is filtered out.
 *
 * The button fetches what the table is showing, so a label reading "all" while a chip hides most of
 * the tree would be a lie about how many network operations one click starts — and the intended
 * workflow is exactly that: filter to stale, then fetch what is left.
 */
const fetchLabel = computed(() => {
  const scope = props.fetchable === props.total ? 'all' : 'shown';
  return `Fetch ${scope} (${props.fetchable})`;
});

/**
 * A disabled control has to say why, and there are two reasons it can be disabled.
 */
const fetchTitle = computed(() => {
  if (props.gitMissing !== null) return props.gitMissing;
  if (props.fetchable === 0) return 'There are no repositories shown to fetch.';
  return 'Run `git fetch` in every repository the table is showing.';
});
</script>
