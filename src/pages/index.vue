<template>
  <div class="flex min-h-0 w-full flex-1 flex-col">
    <root-bar
      :roots="repos.roots"
      :scanning="repos.scanning"
      :disabled="!bridgeReady"
      :fetchable="repoView.visible"
      :total="repos.count"
      :fetching="repos.fetching"
      :git-missing="gitMissing"
      @pick="PickAndAdd"
      @scan="Scan"
      @cancel="Cancel"
      @remove="Remove"
      @fetch="Fetch"
      @cancel-fetch="StopFetch"
    />

    <filter-bar
      :chips="view.chips"
      :query="view.query"
      :group-by-folder="view.groupByFolder"
      :counts="repoView"
      :total="repos.count"
      :disabled="!bridgeReady"
      @chip="view.ToggleChip"
      @update:query="view.SetQuery"
      @update:group-by-folder="view.SetGroupByFolder"
      @clear="view.ClearFilters"
    />

    <div v-if="repos.scanError" class="px-20 pt-10">
      <app-alert tone="error" title="The scan failed">{{ repos.scanError }}</app-alert>
    </div>

    <!--
      A warning and not an error: watching is an optimisation, and the 60-second poll and the
      refresh-on-focus underneath it are still running. So the title says what a user now gets
      rather than that something broke.
    -->
    <div v-if="repos.watchError" class="px-20 pt-10">
      <app-alert tone="warning" title="Rows update on a timer, not instantly">
        {{ repos.watchError }}
      </app-alert>
    </div>

    <!--
      Reading status needs no `git` at all, so this says what is unavailable rather than that
      something is wrong. An `info` tone and a line on the page, not only a tooltip on a disabled
      button: a tooltip is unreachable by keyboard and invisible to anyone who does not hover,
      which is not §10.2's "with an explanation".
    -->
    <div v-if="gitMissing" class="px-20 pt-10">
      <app-alert tone="info" title="Fetching is unavailable">{{ gitMissing }}</app-alert>
    </div>

    <scan-progress
      v-if="repos.phase !== 'idle'"
      :progress="repos.progress"
      :discovery="repos.discovery"
      :totals="repos.totals"
      :repo-errors="repos.repoErrors"
    />

    <!--
      Its own `v-if`, not folded into `ScanProgress`: that one is gated on a scan phase, and a
      fetch has none. It stays up after the pass so the failures remain readable.
    -->
    <fetch-progress
      v-if="fetchProgress !== null"
      :progress="fetchProgress"
      :fetch-errors="repos.fetchErrors"
    />

    <repo-table
      :groups="repoView.groups"
      :sort-key="view.sortKey"
      :sort-direction="view.sortDirection"
      :now="now"
      :repo-errors="repos.repoErrors"
      :tier0-done="repos.tier0Done"
      :expanded="repos.expanded"
      :loading-detail="repos.loadingDetail"
      :detail-errors="repos.detailErrors"
      :open-errors="repos.openErrors"
      :fetch-states="repos.fetchStates"
      :fetch-errors="repos.fetchErrors"
      :git-missing="gitMissing"
      :empty-message="emptyMessage"
      @toggle="ToggleRow"
      @refresh="RefreshDetail"
      @open="OpenIn"
      @fetch="FetchRow"
      @sort="view.SortBy"
    />
  </div>
</template>

<script setup lang="ts">
import { useNow, watchDebounced } from '@vueuse/core';
import { computed, inject, ref, watch } from 'vue';
import AppAlert from '@/components/feedback/AppAlert.vue';
import FetchProgress from '@/components/feedback/FetchProgress.vue';
import ScanProgress from '@/components/feedback/ScanProgress.vue';
import FilterBar from '@/components/repos/FilterBar.vue';
import RepoTable from '@/components/repos/RepoTable.vue';
import RootBar from '@/components/repos/RootBar.vue';
import { OpenIn, RefreshDetail, ToggleRow } from '@/scripts/detail';
import { CancelFetch, LoadGitInfo, StartFetch } from '@/scripts/fetch';
import * as ipc from '@/scripts/ipc';
import { CancelScan, ReconcileOnLaunch, StartScan } from '@/scripts/scan';
import { indexVersion, searchPaths } from '@/scripts/search';
import { parseUiSettings } from '@/scripts/settings';
import { buildView } from '@/scripts/view';
import { type FetchProgress as FetchProgressState, useReposStore } from '@/stores/repos';
import { useViewStore } from '@/stores/view';

/// Composed
const repos = useReposStore();
const view = useViewStore();

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

/// Data
/**
 * The view as last written to disk, serialised.
 *
 * What the persisting watcher compares against, and it earns its place twice over. `view.settings`
 * is a `computed` that builds a new object every evaluation, so a watcher on it fires on any
 * dependency change rather than on a real one — and {@link Hydrate} applying the saved view is
 * itself such a change, which would make the first thing every launch does a write-back of what it
 * just read.
 */
let persisted = '';

/**
 * Whether {@link Hydrate} has been started. Not a `ref`: nothing renders it, and it exists only to
 * keep the watcher below from running twice.
 */
let hydrating = false;

/**
 * How many repositories the current fetch was asked for.
 *
 * Captured at the click rather than derived from the store, because the store's counts shrink as
 * results land — a denominator taken from them would fall towards the numerator and the bar would
 * never move.
 */
const fetchTotal = ref(0);

/// Computed
/**
 * The clock as epoch milliseconds, which is what every row field is measured in.
 */
const now = computed(() => clock.value.getTime());

/**
 * Paths the search matched, or `null` when there is no query.
 *
 * `indexVersion` is read here rather than inside the search: the index is a `shallowRef` whose
 * mutations notify nothing, so this is where the table declares that its results depend on a corpus
 * that is still streaming in.
 */
const matches = computed(() => searchPaths(view.query, indexVersion.value));

/**
 * Rows a chip must never hide: one being fetched, and one whose fetch failed.
 *
 * Without this the `stale-fetch` chip makes each row vanish at the moment its result arrives, and
 * leaves a failed one looking identical to one not yet reached — hiding the only thing on the
 * screen worth reading.
 */
const busyPaths = computed(() => new Set([...repos.fetchingPaths, ...repos.fetchErrors.keys()]));

/**
 * What the table renders, and what it is withholding.
 */
const repoView = computed(() =>
  buildView(repos.rows, view.settings, now.value, matches.value, busyPaths.value),
);

/**
 * Why fetching is unavailable, or `null` when it is available.
 *
 * `undefined` — not yet asked — is deliberately not "missing": saying so before the answer arrives
 * would disable the controls with an explanation that might be false.
 */
const gitMissing = computed(() =>
  repos.gitInfo === null
    ? 'No usable `git` was found on PATH. Reading repository status needs none; only fetching does.'
    : null,
);

/**
 * Where the fetch has got to, or `null` when none has run this session.
 *
 * Determinate from the first event, because the denominator is the list the frontend handed Rust.
 * Kept up after the pass so its failures stay readable, and cleared only by the next fetch.
 */
const fetchProgress = computed<FetchProgressState | null>(() => {
  const running = [...repos.fetchStates.values()].filter((state) => state === 'running').length;
  const outstanding = repos.fetchStates.size;
  const failed = repos.fetchErrors.size;
  if (outstanding === 0 && failed === 0) return null;

  return {
    total: fetchTotal.value,
    // Settled is the total less what is still queued or running — never a count of what has
    // started, which would read 100% with four fetches left to finish.
    settled: Math.max(fetchTotal.value - outstanding, 0),
    running,
    failed,
  };
});

/**
 * What the table says when it has no rows, which depends on why it has none.
 *
 * The filtered case comes first among the ones that can coexist: with rows in the store and none on
 * screen, the filters are the explanation, and "No repositories found under the configured folders"
 * would blame the tree for something the toolbar did.
 */
const emptyMessage = computed(() => {
  if (repos.count > 0 && view.filtering) return 'No repositories match the current filters.';
  if (repos.roots.length === 0) return 'Add a folder to scan.';
  if (repos.phase === 'idle') return 'Ready. Press Scan.';
  if (repos.scanning) return 'Searching…';
  return 'No repositories found under the configured folders.';
});

/// Watchers
/**
 * Hydrates as soon as the bridge is up, and not before.
 *
 * A watcher rather than `onMounted`, because a child's `onMounted` runs **before** its parent's:
 * the layout has not finished its `ping` by the time this page mounts, so `bridgeReady` is still
 * false here and a mount-time check would give up on a bridge that was merely still connecting.
 * With persisted roots that is the difference between a launch that reconciles and one that shows
 * an empty table until the user presses Scan.
 *
 * `immediate` for the case where the bridge is already up — a route change back to this page — and
 * guarded, so neither path can run it twice.
 */
watch(
  bridgeReady,
  (ready) => {
    if (ready && !hydrating) {
      hydrating = true;
      void Hydrate();
    }
  },
  { immediate: true },
);

/**
 * Persists the view whenever it differs from what is on disk.
 *
 * Debounced because a chip is often two clicks and a sort three, and each would otherwise be its
 * own command. The delay is invisible: nothing reads this back until the next launch.
 */
watchDebounced(
  () => JSON.stringify(view.settings),
  (next) => {
    if (next === persisted) return;
    persisted = next;
    void Persist();
  },
  { debounce: 400 },
);

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
 * Scans every configured root, starting over.
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

/**
 * Fetches every repository the table is currently showing.
 *
 * The paths are snapshotted at the click, not read as the pass runs: the view changes underneath as
 * results land, and a set that shifted mid-pass would be a different button than the one whose
 * label the user read.
 */
async function Fetch(): Promise<void> {
  const paths = repoView.value.groups.flatMap((group) => group.rows.map((row) => row.path));
  fetchTotal.value = paths.length;
  await StartFetch(paths);
}

/**
 * Fetches one repository.
 *
 * A single path, which Rust exempts from the repeat guard: clicking one row's button is intent,
 * where a batch is where "fetching 300 repositories unprompted" applies.
 *
 * @param path - The repository to fetch.
 */
async function FetchRow(path: string): Promise<void> {
  fetchTotal.value = 1;
  await StartFetch([path]);
}

/**
 * Stops the running fetch.
 */
async function StopFetch(): Promise<void> {
  await CancelFetch();
}

/**
 * Writes the view state.
 *
 * A failure is logged and not shown. The user's click has already taken effect on screen — what
 * failed is only its persistence — so an alert would report a problem they cannot act on about an
 * action that appeared to work.
 */
async function Persist(): Promise<void> {
  try {
    await ipc.saveUiSettings(view.settings);
  } catch (error) {
    console.warn('could not save the view settings', error);
  }
}

/**
 * Applies the saved view, then reconciles the cached rows against the disk.
 *
 * In that order: the settings decide how the rows Rust restored are ordered and filtered, and doing
 * it the other way round would sort the table twice and show the user the wrong one first.
 */
async function Hydrate(): Promise<void> {
  repos.SetRoots(await ipc.listRoots());

  try {
    view.Apply(parseUiSettings(await ipc.uiSettings()));
  } catch (error) {
    // A view that could not be read is not worth a visible failure: the defaults are a working
    // table, which is more than an error message would give.
    console.warn('could not read the saved view settings', error);
  }
  persisted = JSON.stringify(view.settings);

  // Before the reconcile rather than after it: the fetch controls render immediately and would
  // otherwise sit enabled for the length of a scan on a machine with no `git`.
  await LoadGitInfo();

  await ReconcileOnLaunch(repos.roots);
}
</script>
