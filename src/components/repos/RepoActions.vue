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

      <app-button
        :label="fetchLabel"
        icon="bi-cloud-download"
        :title="fetchTitle"
        :disabled="gitMissing !== null || fetchState === 'queued'"
        :busy="fetchState === 'running'"
        variant="outline"
        size="small"
        @click="emit('fetch')"
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

    <!-- A fourth error map, and a title of its own so a failed launch and a failed fetch can never
         be read as the same thing. The message is git's own words wherever there were any. -->
    <app-alert v-if="fetchError !== null" tone="warning" title="Could not fetch">
      {{ fetchError }}
    </app-alert>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import AppAlert from '@/components/feedback/AppAlert.vue';
import AppButton from '@/components/inputs/AppButton.vue';
import type { OpenTarget } from '@/scripts/ipc';
import type { FetchRowState } from '@/stores/repos';

/**
 * The ways out of the app, and the one way in, for a single repository.
 *
 * Available for **every** row, including one Tier 0 could not read: that is the row a user most
 * wants to go and look at, and revealing it in the file manager is how they find out why it will
 * not open. Fetch is offered there too — a repository whose HEAD is unreadable has no ahead/behind
 * at all, so a fetch is one of the few things that might help. Rust validates all of these against
 * what discovery found rather than against the rows, for exactly that reason.
 */

/// Setup
const props = defineProps<{
  /**
   * Why the last launch failed, or `null`.
   */
  openError: string | null;
  /**
   * Whether a fetch of this row is queued, running, or neither.
   */
  fetchState: FetchRowState | null;
  /**
   * Why the last fetch of this row failed, or `null`.
   */
  fetchError: string | null;
  /**
   * Why fetching is unavailable, or `null` when it is available.
   */
  gitMissing: string | null;
}>();

const emit = defineEmits<{
  /**
   * A launch button was pressed.
   */
  open: [target: OpenTarget];
  /**
   * The fetch button was pressed.
   */
  fetch: [];
}>();

/// Computed
/**
 * "Queued" is a state worth naming rather than a spinner: with a concurrency cap a repository can
 * wait minutes before its process starts, and a spinner for the whole wait would claim work that is
 * not happening yet.
 */
const fetchLabel = computed(() => (props.fetchState === 'queued' ? 'Queued' : 'Fetch'));

/**
 * A disabled button has to say why. The tooltip is not the whole answer — the page carries the
 * explanation as well, because a tooltip is unreachable by keyboard — but a button that refuses
 * silently is worse than either.
 */
const fetchTitle = computed(() => {
  if (props.gitMissing !== null) return props.gitMissing;
  return 'Run `git fetch` here, so the ahead and behind counts are current.';
});

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
