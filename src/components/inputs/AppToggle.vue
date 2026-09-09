<template>
  <toggle v-model="model" :disabled="disabled" as-child>
    <button type="button" :class="classes" :title="title || undefined">
      <i v-if="icon" :class="`bi ${icon} mr-6`" />
      <span v-text="label" />
      <span v-if="count !== null" class="text-11 ml-6 text-gray-600" v-text="count" />
    </button>
  </toggle>
</template>

<script setup lang="ts">
import { Toggle } from 'reka-ui';
import { computed } from 'vue';

/**
 * A two-state button — pressed or not.
 *
 * Wraps `reka-ui`'s `Toggle`, which is the primitive for exactly this: it renders a real `button`
 * carrying `aria-pressed` and a `data-state`, so a filter chip announces itself correctly instead
 * of looking like a button that happens to be a different colour.
 *
 * The styling reuses the `btn` utility so a chip sits beside the toolbar's buttons without a second
 * visual language: pressed reads as primary, unpressed as outline.
 *
 * `as-child` so the rendered element is our own button. `Toggle` merges its state, its
 * `aria-pressed` and its click handling onto the child, and everything a plain button already
 * accepts — a `title` here — stays available. Without it a tooltip would have to be a fallthrough
 * attribute, which `vue-tsc` checks against the primitive's declared props and rejects.
 */

/// Setup
const model = defineModel<boolean>({ required: true });

const props = withDefaults(
  defineProps<{
    /**
     * The chip's text. Always present — an icon alone is not a label.
     */
    label: string;
    /**
     * A bootstrap-icons class, e.g. `bi-pencil`.
     */
    icon?: string;
    /**
     * A number shown after the label, or `null` for none.
     *
     * `null` and not `0`: a count of zero is a fact worth showing, and a chip with nothing to say
     * about its count is a different thing entirely.
     */
    count?: number | null;
    /**
     * A tooltip, for a chip whose label cannot say the whole rule.
     */
    title?: string;
    /**
     * Whether the chip is unavailable.
     */
    disabled?: boolean;
  }>(),
  { icon: '', count: null, title: '', disabled: false },
);

/// Computed
/**
 * `btn` plus the companion class for the current state.
 *
 * The companions are nested inside `@utility btn` in `main.css`, so they only take effect alongside
 * the base class — composing them in one place is why this wrapper exists at all.
 */
const classes = computed(() => [
  'btn btn-small',
  model.value ? 'btn-primary' : 'btn-outline',
  props.disabled ? '' : 'cursor-pointer',
]);
</script>
