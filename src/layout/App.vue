<template>
  <app-header />
  <main class="absolute inset-0 top-50 flex flex-col overflow-hidden bg-gray-100">
    <!-- A failed bridge is the interesting case: it would otherwise present as a page that simply
         renders nothing. -->
    <div v-if="boot === 'failed'" class="p-20">
      <app-alert tone="error" title="The backend is unavailable">
        {{ bootError }}
      </app-alert>
    </div>

    <router-view />
  </main>
</template>

<script setup lang="ts">
import { computed, onMounted, provide, ref } from 'vue';
import AppAlert from '@/components/feedback/AppAlert.vue';
import { ping } from '@/scripts/ipc';
import { StartSession } from '@/scripts/scan';
import AppHeader from './Header.vue';
import '@/styles/main.css';

/// Type
/**
 * Where the backend check has got to. Three states rather than a boolean: "not answered yet" and
 * "answered badly" are different things to show.
 */
type BootState = 'pending' | 'ready' | 'failed';

/// Data
const boot = ref<BootState>('pending');
const bootError = ref('');

/// Computed
/**
 * Whether commands can be called at all. Pages disable their controls on this.
 */
const bridgeReady = computed(() => boot.value === 'ready');

/// Methods
/**
 * Checks the bridge, then opens the session channel.
 *
 * `ping` first, deliberately. It is the cheapest possible round trip, so a failure here means IPC
 * itself is down — which is a different diagnosis from `subscribe` failing, and worth telling
 * apart.
 *
 * The session channel is opened here rather than in a page because it lives for the app session: a
 * route component can unmount, and the channel must not go with it.
 */
async function Boot(): Promise<void> {
  try {
    await ping();
    await StartSession();
    boot.value = 'ready';
  } catch (error) {
    bootError.value = error instanceof Error ? error.message : String(error);
    boot.value = 'failed';
  }
}

/// Lifecycle
provide('bridgeReady', bridgeReady);
onMounted(Boot);
</script>
