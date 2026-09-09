<template>
  <!-- `h-32` is a floor, not a fixed height: CSS treats `height` on a table row as a minimum, and
       the Upstream-and-sync cell runs to three lines whenever there are counts and a fetch age to
       show. So a no-upstream row is 32px and most rows are taller. Virtualizing this later needs
       dynamic measurement — see PLAN §3.2. -->
  <tr :class="['border-gray-150 h-32 border-b', rowTone]">
    <!-- Repository -->
    <td class="px-10">
      <div class="flex items-center gap-8">
        <!-- Only a row with a status has anything to expand: Tier 2 fills fields on a `RepoStatus`,
             and Rust refuses the command without one. A failed row keeps its place in the layout
             rather than shifting its name left. -->
        <button
          v-if="status"
          class="text-11 flex h-16 w-16 items-center justify-center rounded text-gray-500 hover:bg-gray-100 hover:text-gray-700"
          type="button"
          :aria-expanded="expanded"
          :title="expanded ? 'Hide details' : 'Show file counts and submodules'"
          @click="emit('toggle')"
        >
          <i :class="['bi', expanded ? 'bi-chevron-down' : 'bi-chevron-right']" />
        </button>
        <span v-else class="h-16 w-16" />

        <i
          v-if="failed"
          class="bi bi-x-octagon-fill text-red-600"
          :title="readError ?? 'This repository could not be read.'"
        />
        <i
          v-else-if="status?.error"
          class="bi bi-exclamation-triangle-fill text-orange-600"
          :title="status.error"
        />
        <span class="text-13 font-medium" v-text="row.name" />
        <span
          v-if="row.kind !== 'normal'"
          class="text-11 rounded bg-gray-100 px-6 text-gray-600"
          v-text="KIND_LABEL[row.kind]"
        />
      </div>
      <div class="text-11 text-gray-500" v-text="row.parent" />
    </td>

    <!-- Head -->
    <td class="px-10">
      <repo-head v-if="status" :head="status.head" />
      <app-unknown v-else :reason="absent" :hint="readError" />
    </td>

    <!-- Upstream and sync -->
    <td class="px-10">
      <ahead-behind
        v-if="status"
        :ahead="status.ahead"
        :behind="status.behind"
        :upstream="status.upstream"
        :last-fetched-ms="status.lastFetchedMs"
        :now="now"
      />
      <app-unknown v-else :reason="absent" :hint="readError" />
    </td>

    <!-- State -->
    <td class="px-10">
      <repo-state-badge v-if="status" :state="status.state" />
      <app-unknown v-else :reason="absent" :hint="readError" />
    </td>

    <!-- Stash -->
    <td class="px-10 text-right">
      <span v-if="status" class="text-12">
        <span v-text="status.stashCount" />
        <!-- A count is only trustworthy on a row without an error: stashCount has to report a
             number, so a failed read reports 0 and sets `error`. -->
        <span
          v-if="status.error"
          class="text-gray-450"
          title="This count may be wrong: the row reported an error."
          v-text="'*'"
        />
      </span>
      <app-unknown v-else :reason="absent" :hint="readError" />
    </td>

    <!-- Worktree -->
    <td class="px-10">
      <template v-if="status">
        <!-- A bare repository has no worktree, so this is not pending — it can never apply. -->
        <app-unknown
          v-if="status.kind === 'bare'"
          reason="na"
          hint="Bare repository — no worktree to compare against."
        />
        <app-unknown v-else-if="status.dirty === null" reason="pending" />
        <span v-else-if="status.dirty" class="text-12 flex items-center gap-8">
          <span class="text-orange-600" v-text="'dirty'" />
          <span
            v-if="status.conflicted !== null && status.conflicted > 0"
            class="text-red-600"
            v-text="`${status.conflicted} conflicted`"
          />
        </span>
        <span v-else class="text-12 text-green-600" v-text="'clean'" />
      </template>
      <app-unknown v-else :reason="absent" :hint="readError" />
    </td>

    <!-- Read -->
    <td class="px-10 text-right">
      <span
        v-if="status"
        class="text-11 text-gray-500"
        v-text="formatAge(status.scannedAtMs, now)"
      />
      <app-unknown v-else :reason="absent" :hint="readError" />
    </td>
  </tr>

  <!-- The drawer, as a second row rather than inside the first: a `<td>` cannot contain a block
       that spans the table, and `colspan` is how a table says "full width". A plain `v-if` and not
       a `reka-ui` Collapsible — that primitive wraps its content in elements of its own, which are
       not valid between a `<tr>` and its cells. -->
  <tr v-if="status && expanded" :class="rowTone">
    <td :colspan="colspan" class="p-0">
      <repo-detail
        :row="status"
        :loading="loadingDetail"
        :detail-error="detailError"
        :now="now"
        @refresh="emit('refresh')"
      />
    </td>
  </tr>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import AppUnknown from '@/components/feedback/AppUnknown.vue';
import AheadBehind from '@/components/repos/AheadBehind.vue';
import RepoDetail from '@/components/repos/RepoDetail.vue';
import RepoHead from '@/components/repos/RepoHead.vue';
import RepoStateBadge from '@/components/repos/RepoStateBadge.vue';
import type { RepoKind } from '@/scripts/generated/RepoKind';
import { formatAge } from '@/scripts/utils';
import { type RepoRow, isRead } from '@/stores/repos';

/// Setup
const props = defineProps<{
  /**
   * The row, whether or not Tier 0 has read it.
   */
  row: RepoRow;
  /**
   * The current time, for the age columns.
   */
  now: number;
  /**
   * Why this repository could not be read, when Tier 0 reported one.
   */
  readError: string | null;
  /**
   * Whether Tier 0 has finished, which is what turns "not yet" into "never".
   */
  tier0Done: boolean;
  /**
   * Whether this row's detail drawer is open.
   */
  expanded: boolean;
  /**
   * Whether a Tier 2 read is in flight for this row.
   */
  loadingDetail: boolean;
  /**
   * Why this row's last Tier 2 read failed, if it did.
   */
  detailError: string | null;
  /**
   * How many columns the table has, for the drawer's `colspan`.
   *
   * Passed rather than restated here: `RepoTable` owns the column list, and a second copy of its
   * length would be a literal that drifts the moment a column is added — with a drawer that stops
   * spanning the table as the only symptom.
   */
  colspan: number;
}>();

const emit = defineEmits<{
  /**
   * The user clicked the expander.
   */
  toggle: [];
  /**
   * The user asked the drawer for a fresh read.
   */
  refresh: [];
}>();

/// Computed
/**
 * The row as a read status, or `null` when discovery is all that has happened.
 *
 * The one `isRead` branch in the whole component tree. Everything below it is handed `RepoStatus`
 * fields, so no cell component can be given a row that does not have them.
 */
const status = computed(() => (isRead(props.row) ? props.row : null));

/**
 * Whether this repository will never produce a row.
 *
 * Two ways to know, and either is enough. Rust reports a total failure **as the batch that failed
 * is read**, so a row can be known-unreadable while the scan is still running — that is the first
 * clause, and it is what stops a broken repository claiming to be "counting…" for the minute or
 * more a large tree takes. The second clause catches the rest: once Tier 0 has finished, a row that
 * is still only a `DiscoveredRepo` is not waiting for anything either, whether or not its cause
 * arrived.
 *
 * Saying `counting…` in either case would be exactly the dishonesty the unknown-is-not-zero rule
 * exists to prevent, pointed at time instead of at counts.
 */
const failed = computed(
  () => status.value === null && (props.readError !== null || props.tier0Done),
);

/**
 * Which absence the tiered cells show, before Tier 0 finishes and after.
 */
const absent = computed(() => (failed.value ? ('unreadable' as const) : ('pending' as const)));

/**
 * A failed row is tinted red, a partial one orange.
 */
const rowTone = computed(() => {
  if (failed.value) return 'bg-red-50';
  return status.value?.error ? 'bg-orange-50' : '';
});

/// Data
/**
 * How the non-ordinary repository kinds are labelled.
 */
const KIND_LABEL: Record<RepoKind, string> = {
  normal: '',
  bare: 'bare',
  linkedWorktree: 'worktree',
  submodule: 'submodule',
};
</script>
