<template>
  <div class="flex min-h-0 w-full flex-1 flex-col">
    <root-bar
      :roots="repos.roots"
      :scanning="repos.scanning"
      :disabled="!bridgeReady"
      @pick="PickAndAdd"
      @scan="Scan"
      @cancel="Cancel"
      @remove="Remove"
    />

    <div v-if="repos.scanError" class="px-20 pt-10">
      <app-alert tone="error" title="The scan failed">{{ repos.scanError }}</app-alert>
    </div>

    <scan-progress
      v-if="repos.phase !== 'idle'"
      :progress="repos.progress"
      :discovery="repos.discovery"
      :totals="repos.totals"
      :repo-errors="repos.repoErrors"
    />

    <repo-table
      :rows="repos.rows"
      :now="now"
      :repo-errors="repos.repoErrors"
      :tier0-done="repos.tier0Done"
      :expanded="repos.expanded"
      :loading-detail="repos.loadingDetail"
      :detail-errors="repos.detailErrors"
      :empty-message="emptyMessage"
      @toggle="ToggleRow"
      @refresh="RefreshDetail"
    />
  </div>
</template>

<script setup lang="ts">
import { useNow } from '@vueuse/core';
import { computed, inject, onMounted, ref } from 'vue';
import AppAlert from '@/components/feedback/AppAlert.vue';
import ScanProgress from '@/components/feedback/ScanProgress.vue';
import RepoTable from '@/components/repos/RepoTable.vue';
import RootBar from '@/components/repos/RootBar.vue';
import { RefreshDetail, ToggleRow } from '@/scripts/detail';
import * as ipc from '@/scripts/ipc';
import { CancelScan, StartScan } from '@/scripts/scan';
import { useReposStore } from '@/stores/repos';

/// Composed
const repos = useReposStore();

/**
 * One clock for the whole page, passed down as a prop.
 *
 * `useTimeAgo` per value would allocate a timer per cell — at 500 rows across two age columns that
 * is a thousand of them. A single ticking value plus pure formatters is cheaper and testable with a
 * frozen clock.
 */
const clock = useNow({ interval: 30_000 });

/**
 * Whether the IPC bridge came up, provided by the layout.
 */
const bridgeReady = inject('bridgeReady', ref(true));

/// Computed
/**
 * The clock as epoch milliseconds, which is what every row field is measured in.
 */
const now = computed(() => clock.value.getTime());

/**
 * What the table says when it has no rows, which depends on why it has none.
 */
const emptyMessage = computed(() => {
  if (repos.roots.length === 0) return 'Add a folder to scan.';
  if (repos.phase === 'idle') return 'Ready. Press Scan.';
  if (repos.scanning) return 'Searching…';
  return 'No repositories found under the configured folders.';
});

/// Methods
/**
 * Opens the folder picker and adds whatever the user chose.
 */
async function PickAndAdd(): Promise<void> {
  try {
    const picked = await ipc.pickRoot();
    if (picked === null) return;
    repos.SetRoots(await ipc.addRoot(picked));
  } catch (error) {
    repos.SetScanError(error instanceof Error ? error.message : String(error));
  }
}

/**
 * Removes one root. Its rows are evicted by Rust and arrive on the session channel.
 *
 * @param root - The root to remove.
 */
async function Remove(root: string): Promise<void> {
  try {
    repos.SetRoots(await ipc.removeRoot(root));
  } catch (error) {
    repos.SetScanError(error instanceof Error ? error.message : String(error));
  }
}

/**
 * Scans every configured root.
 */
async function Scan(): Promise<void> {
  await StartScan(repos.roots);
}

/**
 * Stops the running scan.
 */
async function Cancel(): Promise<void> {
  await CancelScan();
}

/// Lifecycle
onMounted(async () => {
  if (!bridgeReady.value) return;
  repos.SetRoots(await ipc.listRoots());
});
</script>
