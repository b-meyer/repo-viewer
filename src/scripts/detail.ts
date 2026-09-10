/**
 * Expanding a row: when Tier 2 runs, and when it does not.
 *
 * A module of plain functions rather than a composable, matching `scan.ts`: components call domain
 * actions and never reach `ipc.ts` themselves, so the one decision worth centralising — whether
 * this expand needs a read at all — has exactly one home.
 *
 * # Expanding twice must not read twice
 *
 * Tier 2 is the expensive tier, and Rust keeps what it read: `counts` and `submodules` are fields
 * of the canonical row, so the store's mirror still has them after the drawer closes. Re-expanding
 * therefore paints from the mirror and issues no command. That is the phase's stated deliverable,
 * and it falls out of the store being a mirror rather than out of a cache kept here.
 *
 * The corollary is that a stale count needs an explicit re-read, which is {@link RefreshDetail} —
 * and its `Tier.Two` is cumulative, so it re-reads the refs and the worktree too rather than
 * pairing fresh counts with a stale branch.
 *
 * # A changed repository is the other way counts go missing
 *
 * A watcher refresh drops Tier 2 for a repository that changed underneath an open drawer, because
 * those counts are shown with no age beside them and would otherwise read as freshly measured. That
 * makes `counts` go back to `null` on a row nobody touched, and `counting…` is a claim about work
 * in progress — so {@link EnsureDetail} exists to make the claim true, and `scan.ts` calls it for
 * the expanded rows in every update it applies.
 */
import type { RepoStatus } from '@/scripts/generated/RepoStatus';
import * as ipc from '@/scripts/ipc';
import type { OpenTarget } from '@/scripts/ipc';
import { isRead, useReposStore } from '@/stores/repos';

/// Methods
/**
 * Toggles a row's drawer, reading Tier 2 the first time it opens.
 *
 * @param path - The row to toggle.
 */
export async function ToggleRow(path: string): Promise<void> {
  const repos = useReposStore();
  if (!repos.ToggleExpanded(path)) return;

  await EnsureDetail(path);
}

/**
 * Reads Tier 2 for a row if it needs reading, and does nothing if it does not.
 *
 * The one place that decides whether an expand costs a command. Two callers, and the second is what
 * keeps `counting…` honest: a watcher refresh invalidates Tier 2 for a repository that changed, so
 * `counts` can go back to `null` **while the drawer is open**. Nothing would then re-read it, and
 * the drawer would claim work was in progress for the rest of the session — the false-activity bug
 * this whole vocabulary exists to prevent. So `scan.ts` calls this for every expanded row in an
 * update it applies, and the guards below make that call free whenever there is nothing to do.
 *
 * @param path - The row to read.
 */
export async function EnsureDetail(path: string): Promise<void> {
  const repos = useReposStore();

  const row = repos.byPath.get(path);
  // A row discovery found but Tier 0 could not read has no `RepoStatus` to fill in, and Rust
  // refuses the command for exactly that reason. Opening the drawer is still allowed — it is where
  // the failure is explained.
  if (row === undefined || !isRead(row)) return;
  // A bare repository has no worktree to diff and no `.gitmodules`, so both halves are `n/a`
  // forever. Rust answers correctly if asked, but the answer can never populate `counts` — so the
  // "already read" guard below would never fire and every single expand would spend a round trip
  // to be told the same nothing.
  if (row.kind === 'bare') return;
  // Already read, and Rust has not forgotten it. Nothing to ask for.
  if (row.counts !== null) return;
  // A read is already in flight, which is the case a watcher push lands in most often: the row
  // arrives, this runs, and the update that follows must not start a second read of the same thing.
  if (repos.loadingDetail.has(path)) return;

  await Read(path, () => ipc.fullStatus(path));
}

/**
 * Re-reads one row from refs to counts, discarding what is already there.
 *
 * `Tier.Two` and not a Tier 2-only read: the counts describe a worktree relative to a branch, so
 * refreshing them alone would pair a fresh number with a stale ref and present the pair as one
 * moment.
 *
 * @param path - The row to re-read.
 */
export async function RefreshDetail(path: string): Promise<void> {
  await Read(path, () => ipc.refreshRepo(path, 'two'));
}

/**
 * Opens one row in an external tool.
 *
 * Not a read, so it touches none of the tier state: the failure it can produce is a launch failure
 * and lands in its own map, cleared by the next attempt. There is nothing to show while it runs —
 * the answer is a window appearing somewhere else — so there is no busy state either.
 *
 * @param path - The row whose button was pressed.
 * @param target - Where to open it.
 */
export async function OpenIn(path: string, target: OpenTarget): Promise<void> {
  const repos = useReposStore();
  repos.SetOpenError(path, null);

  try {
    await ipc.openIn(path, target);
  } catch (error) {
    repos.SetOpenError(path, error instanceof Error ? error.message : String(error));
  }
}

/**
 * Runs one per-row read, recording that it is in flight and what came of it.
 *
 * The `finally` is what keeps `counting…` honest: it is a claim that work is happening, so it has
 * to stop the moment the work does, on the failure path as much as on the successful one.
 *
 * @param path - The row being read.
 * @param read - The command to run.
 */
async function Read(path: string, read: () => Promise<unknown>): Promise<void> {
  const repos = useReposStore();
  repos.SetLoadingDetail(path, true);
  repos.SetDetailError(path, null);

  try {
    // The merged row arrives through the command's own reply for `fullStatus` and on the session
    // channel for `refreshRepo`. Neither is applied here: `scan.ts` owns the session channel, and a
    // command reply is mirrored the same way a scan batch is.
    const row = await read();
    if (isRepoStatus(row)) repos.Upsert(row);
  } catch (error) {
    repos.SetDetailError(path, error instanceof Error ? error.message : String(error));
  } finally {
    repos.SetLoadingDetail(path, false);
  }
}

/**
 * Whether a command reply is a row.
 *
 * Both commands here resolve with a `RepoStatus`, but `invoke` is typed by assertion rather than by
 * validation, so this narrows before the value reaches the store — which is the one place a shape
 * from outside would otherwise become a row nobody checked.
 *
 * @param value - The command's reply.
 * @returns Whether it can be mirrored as a row.
 */
function isRepoStatus(value: unknown): value is RepoStatus {
  return typeof value === 'object' && value !== null && 'head' in value && 'path' in value;
}
