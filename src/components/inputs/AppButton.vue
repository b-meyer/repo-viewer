<template>
  <button :class="classes" :disabled="disabled || busy" type="button" @click="emit('click')">
    <i v-if="busy" class="bi bi-arrow-repeat mr-6 animate-spin" />
    <i v-else-if="icon" :class="`bi ${icon} mr-6`" />
    <span v-text="label" />
  </button>
</template>

<script setup lang="ts">
import { computed } from 'vue';

/// Setup
const props = withDefaults(
  defineProps<{
    /**
     * The button's text. Always present — an icon alone is not a label.
     */
    label: string;
    /**
     * Which of `main.css`'s `btn` companions to apply.
     */
    variant?: 'default' | 'primary' | 'outline';
    /**
     * Height and padding.
     */
    size?: 'default' | 'small';
    /**
     * A bootstrap-icons class, e.g. `bi-folder-plus`.
     */
    icon?: string;
    /**
     * Whether the action is unavailable.
     */
    disabled?: boolean;
    /**
     * Whether the action is running. Implies disabled, and swaps the icon for a spinner.
     */
    busy?: boolean;
  }>(),
  { variant: 'default', size: 'default', icon: '', disabled: false, busy: false },
);

const emit = defineEmits<{
  /**
   * The button was activated.
   */
  click: [];
}>();

/// Computed
/**
 * `btn` plus its companion classes.
 *
 * `main.css` defines `btn-primary` and friends nested inside `@utility btn`, so they only take
 * effect _alongside_ `btn` — dropping the base class silently loses every rule. Composing it in one
 * place is the reason this wrapper exists.
 */
const classes = computed(() => [
  'btn',
  props.variant === 'primary' && 'btn-primary',
  props.variant === 'outline' && 'btn-outline',
  props.size === 'small' && 'btn-small',
]);
</script>
