<template>
  <div class="flex flex-col gap-6">
    <div class="flex flex-wrap items-center gap-8">
      <app-button
        v-for="action in ACTIONS"
        :key="action.target"
        :label="action.label"
        :icon="action.icon"
        :title="action.title"
        variant="outline"
        size="small"
        @click="emit('open', action.target)"
      />
      <!-- Whatever else belongs in this row of buttons — the drawer puts "Re-read" here, and a row
           with no status has nothing to re-read. -->
      <slot />
    </div>

    <!-- A launch failure is per row and per button, and it is cleared by the next attempt. It is not
         a read failure and not a page-wide one, which is why it arrives in a map of its own. -->
    <app-alert v-if="openError !== null" tone="warning" title="Could not open">
      {{ openError }}
    </app-alert>
  </div>
</template>

<script setup lang="ts">
import AppAlert from '@/components/feedback/AppAlert.vue';
import AppButton from '@/components/inputs/AppButton.vue';
import type { OpenTarget } from '@/scripts/ipc';

/**
 * The three ways out of the app, for one repository.
 *
 * Available for **every** row, including one Tier 0 could not read: that is the row a user most
 * wants to go and look at, and revealing it in the file manager is how they find out why it will
 * not open. Rust validates these against what discovery found rather than against the rows, for
 * exactly that reason.
 */

/// Setup
defineProps<{
  /**
   * Why the last launch failed, or `null`.
   */
  openError: string | null;
}>();

const emit = defineEmits<{
  /**
   * A button was pressed.
   */
  open: [target: OpenTarget];
}>();

/// Data
/**
 * The buttons, in order of how often they are wanted.
 *
 * The editor and the terminal run a command from the settings file; revealing goes through the
 * platform's own shell API and has nothing to configure, which is why only the first two can fail
 * for want of configuration.
 */
const ACTIONS: { target: OpenTarget; label: string; icon: string; title: string }[] = [
  {
    target: 'editor',
    label: 'Editor',
    icon: 'bi-code-slash',
    title: 'Open in the editor configured in settings.json.',
  },
  {
    target: 'terminal',
    label: 'Terminal',
    icon: 'bi-terminal',
    title: 'Open a terminal here, using the command configured in settings.json.',
  },
  {
    target: 'fileManager',
    label: 'Reveal',
    icon: 'bi-folder2-open',
    title: 'Show this folder in the file manager.',
  },
];
</script>
