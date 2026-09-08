<template>
  <div class="flex min-h-0 w-full flex-1 flex-col overflow-auto p-20">
    <div class="mx-auto flex w-full max-w-800 flex-col gap-20">
      <h1 class="text-24 mt-10 font-bold" v-text="'Repo Viewer'" />

      <div class="card flex flex-col gap-10 p-20">
        <h2 class="text-14 font-semibold" v-text="'Backend'" />

        <p v-if="state === 'pending'" class="text-14 text-gray-600" v-text="'Checking…'" />

        <!-- A failed ping is the interesting case: it means the IPC bridge is not wired, which
             would otherwise present as a page that simply renders nothing. -->
        <p v-else-if="state === 'failed'" class="text-14 text-red-600">
          IPC unavailable — {{ error }}
        </p>

        <p v-else class="text-14 text-gray-800">
          Connected. The backend replied <code class="font-mono" v-text="reply" />.
        </p>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ping } from '@/scripts/ipc';

/// Type
/**
 * Where the backend check has got to. Deliberately three states rather than a boolean: "not
 * answered yet" and "answered badly" are different things to show.
 */
type PingState = 'pending' | 'connected' | 'failed';

/// Data
const state = ref<PingState>('pending');
const reply = ref('');
const error = ref('');

/// Methods
/**
 * Calls the backend and records the outcome.
 */
async function Check(): Promise<void> {
  try {
    reply.value = await ping();
    state.value = 'connected';
  } catch (err) {
    error.value = err instanceof Error ? err.message : String(err);
    state.value = 'failed';
  }
}

/// Lifecycle
onMounted(Check);
</script>
