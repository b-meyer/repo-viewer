<template>
  <div class="flex flex-wrap items-center gap-10 border-b border-gray-200 bg-white px-20 py-8">
    <app-input
      :model-value="query"
      class="w-220"
      icon="bi-search"
      placeholder="Search name, path, branch…"
      label="Search repositories"
      :disabled="disabled"
      @update:model-value="emit('update:query', $event)"
    />

    <div class="flex flex-wrap items-center gap-6" role="group" aria-label="Filters">
      <app-toggle
        v-for="chip in FILTER_CHIPS"
        :key="chip"
        :model-value="chips.includes(chip)"
        :label="CHIP_LABEL[chip]"
        :icon="CHIP_ICON[chip]"
        :title="CHIP_TITLE[chip]"
        :disabled="disabled"
        @update:model-value="emit('chip', chip)"
      />
    </div>

    <app-toggle
      :model-value="groupByFolder"
      label="Group by folder"
      icon="bi-folder"
      :disabled="disabled"
      @update:model-value="emit('update:groupByFolder', $event)"
    />

    <!-- The honest half of a filter. A chip that hides rows has to say how many, and separately how
         many of those it could not judge yet — otherwise an uncounted row reads as one that did not
         match, which is the uncomputed-renders-as-zero bug wearing a different hat. -->
    <div v-if="narrowed" class="text-11 ml-auto flex items-center gap-8 text-gray-600">
      <span v-text="`Showing ${counts.visible} of ${total}`" />
      <span v-if="counts.pending > 0" class="text-orange-600">
        {{ counts.pending }} still counting
      </span>
      <app-button
        v-if="filtering"
        label="Clear filters"
        variant="outline"
        size="small"
        icon="bi-x"
        @click="emit('clear')"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import AppButton from '@/components/inputs/AppButton.vue';
import AppInput from '@/components/inputs/AppInput.vue';
import AppToggle from '@/components/inputs/AppToggle.vue';
import { FILTER_CHIPS, type FilterChip } from '@/scripts/settings';
import type { RepoView } from '@/scripts/view';

/**
 * The filter, search, and grouping controls, plus what they are hiding.
 *
 * Prop-driven and store-free, like every other component here: the view state lives in
 * `stores/view.ts` and the page wires the two together, which is what lets each chip's rule be
 * tested against a fixed set of rows.
 */

/// Setup
const props = defineProps<{
  /**
   * The active chips.
   */
  chips: FilterChip[];
  /**
   * What the user has typed.
   */
  query: string;
  /**
   * Whether rows are grouped by folder.
   */
  groupByFolder: boolean;
  /**
   * What the current view is showing and withholding.
   */
  counts: Pick<RepoView, 'visible' | 'hidden' | 'pending'>;
  /**
   * How many rows exist before any narrowing.
   */
  total: number;
  /**
   * Whether the backend is unreachable, which disables everything.
   */
  disabled: boolean;
}>();

const emit = defineEmits<{
  /**
   * A chip was clicked.
   */
  chip: [chip: FilterChip];
  /**
   * The query changed.
   */
  'update:query': [value: string];
  /**
   * Grouping was turned on or off.
   */
  'update:groupByFolder': [value: boolean];
  /**
   * Every filter should be cleared.
   */
  clear: [];
}>();

/// Data
/**
 * What each chip is called.
 */
const CHIP_LABEL: Record<FilterChip, string> = {
  dirty: 'Dirty',
  unpushed: 'Unpushed',
  detached: 'Detached',
  conflicted: 'Conflicted',
  staleFetch: 'Stale fetch',
};

/**
 * A bootstrap-icons class per chip.
 */
const CHIP_ICON: Record<FilterChip, string> = {
  dirty: 'bi-pencil-fill',
  unpushed: 'bi-arrow-up-circle',
  detached: 'bi-signpost-split',
  conflicted: 'bi-exclamation-triangle',
  staleFetch: 'bi-clock-history',
};

/**
 * The rule each chip applies, spelled out.
 *
 * A label cannot carry a qualification, and two of these need one: "unpushed" is relative to the
 * last fetch, and "stale fetch" includes a repository that has never been fetched at all.
 */
const CHIP_TITLE: Record<FilterChip, string> = {
  dirty: 'Uncommitted changes, including untracked files.',
  unpushed: 'Commits the upstream ref did not have as of the last fetch.',
  detached: 'HEAD is not on a branch.',
  conflicted: 'Paths left conflicted by a merge or rebase.',
  staleFetch: 'Fetched over a week ago, or never — so ahead and behind may be out of date.',
};

/// Computed
/**
 * Whether anything is narrowing the table, which is when the counts are worth showing.
 */
const filtering = computed(() => props.chips.length > 0 || props.query.trim() !== '');

/**
 * Whether the table is showing fewer rows than it has.
 *
 * Separate from {@link filtering}: a filter matching everything hides nothing and does not need
 * explaining, and a scan still running can leave rows pending with no filter of its own.
 */
const narrowed = computed(() => filtering.value && props.counts.hidden > 0);
</script>
