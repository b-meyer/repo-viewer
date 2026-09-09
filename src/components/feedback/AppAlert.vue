<template>
  <div :class="['card flex flex-col gap-4 border-l-3 p-15', TONE[tone]]">
    <div class="flex items-center gap-8">
      <i :class="`bi ${ICON[tone]}`" />
      <span class="text-14 font-semibold" v-text="title" />
    </div>
    <div class="text-13 text-gray-700">
      <slot />
    </div>
  </div>
</template>

<script setup lang="ts">
/// Setup
defineProps<{
  /**
   * How serious this is.
   */
  tone: 'error' | 'warning' | 'info';
  /**
   * The headline.
   */
  title: string;
}>();

/// Data
/**
 * Border and background per tone.
 */
const TONE: Record<string, string> = {
  error: 'border-l-red-500 bg-red-50 text-red-700',
  warning: 'border-l-orange-500 bg-orange-50 text-orange-700',
  info: 'border-l-blue-500 bg-blue-50 text-blue-700',
};

/**
 * Leading icon per tone.
 */
const ICON: Record<string, string> = {
  error: 'bi-exclamation-octagon-fill',
  warning: 'bi-exclamation-triangle-fill',
  info: 'bi-info-circle-fill',
};
</script>
