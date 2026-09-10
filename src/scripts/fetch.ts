/**
 * The fetch session: which pass is current, and what its events do to the store.
 *
 * A module-scoped singleton, the same shape as `scan.ts` and `detail.ts` and for the same reasons.
 * A third module rather than a branch in either of those, because a fetch is a third kind of thing:
 * `scan.ts` owns one whole-tree pass and the id race it comes with, `detail.ts` owns one per-row
 * read and its in-flight guard, and this owns a batch of network operations with a lifecycle of its
 * own. Folding it into either would give that module two vocabularies and make its doc block
 * false.
 *
 * # This module never writes a row
 *
 * Rust re-reads each repository it fetched and pushes the merged row on the **session** channel, so
 * `scan.ts`'s `HandleSessionEvent` applies it exactly as it applies a watcher push — including the
 * `EnsureDetail` re-read for an expanded drawer. Nothing here touches `repos.Upsert`, and the
 * session handler needed no change to accommodate fetching.
 *
 * # The same race as a scan, and the same answer
 *
 * `fetchRepos` resolves with its id _after_ Rust may already have sent events, because Rust starts
 * the pass before the invoke's reply crosses back. So the primary filter is a generation counter
 * captured in the handler closure before the invoke, exactly as in `scan.ts`. Unlike a scan there
 * is no id latch on the events: `FetchEvent` carries no id, because two passes are two sets of
 * repositories a user asked for rather than one superseding the other, and every outcome is an
 * idempotent write keyed by path. The id exists to cancel with.
 */
import type { FetchEvent } from '@/scripts/generated/FetchEvent';
import type { FetchId } from '@/scripts/generated/FetchId';
import type { FetchOutcome } from '@/scripts/generated/FetchOutcome';
import type { FetchStatus } from '@/scripts/generated/FetchStatus';
import * as ipc from '@/scripts/ipc';
import { useReposStore } from '@/stores/repos';

/// Data

/**
 * What to say about each outcome when git said nothing, and `null` for the ones that are not
 * failures at all.
 *
 * A `Record` keyed by the union rather than a `switch`, which is the discipline `view.ts` applies
 * to its chips: a status added in Rust and regenerated here becomes a **compile error** at this
 * table rather than falling through to "no failure" — and silently reporting a new kind of failure
 * as a success is the class of bug the tiering rules exist to prevent.
 *
 * Four of the nine are not failures. A repository with nothing to fetch from is an answered
 * question, one skipped by the repeat guard or by a cancellation had nothing attempted, and `ok`
 * succeeded.
 */
const FAILURE_FALLBACK: Record<FetchStatus, string | null> = {
  ok: null,
  noRemote: null,
  tooSoon: null,
  cancelled: null,
  auth: 'Authentication failed.',
  network: 'The remote could not be reached.',
  timedOut: 'The fetch took too long and was stopped.',
  gitMissing: '`git` could not be run.',
  failed: 'The fetch failed.',
};

/**
 * Bumped by every start and every cancel. Events are accepted only while the generation captured in
 * their handler still matches.
 */
let generation = 0;

/**
 * The id of the pass currently being accepted, or `null` when none is.
 */
let activeId: FetchId | null = null;

/**
 * The in-flight `fetchRepos` call, so a cancel arriving before it resolves is not lost.
 */
let pending: Promise<FetchId> | null = null;

/// Methods

/**
 * Starts a fetch over `paths`.
 *
 * Marks every path queued **before** the invoke, so the table says what is about to happen rather
 * than going quiet until Rust answers. `Queued` then confirms the set Rust actually accepted, which
 * can be smaller than the one asked for.
 *
 * @param paths - The repositories to fetch.
 */
export async function StartFetch(paths: string[]): Promise<void> {
  if (paths.length === 0) return;

  const repos = useReposStore();

  generation += 1;
  const mine = generation;
  activeId = null;

  for (const path of paths) {
    repos.SetFetchState(path, 'queued');
    // A new attempt clears the last one's failure: the map is owned by this operation and cleared
    // by its next attempt.
    repos.SetFetchError(path, null);
  }

  const request = ipc.fetchRepos(paths, (event) => {
    HandleFetchEvent(mine, event);
  });
  pending = request;

  try {
    const id = await request;
    if (mine !== generation) {
      // A newer pass started, or a cancel landed, while this one was still being acknowledged.
      // Nothing from it is being accepted, so stop the work too — Rust does not cancel one fetch
      // when another starts, deliberately, so there is nothing else that would.
      await ipc.cancelFetch(id);
      return;
    }
    activeId = id;
  } catch (error) {
    // The command itself was refused. Nothing will ever arrive on the channel, so the queued
    // state has to be unwound here or every row spins for the rest of the session.
    if (mine === generation) {
      repos.ClearFetchStates();
      repos.SetScanError(error instanceof Error ? error.message : String(error));
    }
  } finally {
    if (pending === request) pending = null;
  }
}

/**
 * Stops the current pass.
 *
 * Bumps the generation first, so anything still in flight is inert immediately rather than when
 * Rust gets around to stopping. The rows are cleared here too: the terminal event may never arrive
 * if the cancel raced the pass ending.
 */
export async function CancelFetch(): Promise<void> {
  const repos = useReposStore();

  generation += 1;
  repos.ClearFetchStates();

  const id = activeId ?? (await pending?.catch(() => null)) ?? null;
  activeId = null;
  if (id !== null) await ipc.cancelFetch(id);
}

/**
 * Asks Rust which `git` it found, so the fetch controls can explain themselves.
 *
 * A failure here is not worth surfacing: it means the bridge is down, which `ping` already reports,
 * and treating it as "git is missing" would put a wrong explanation on the button.
 */
export async function LoadGitInfo(): Promise<void> {
  const repos = useReposStore();
  try {
    repos.SetGitInfo(await ipc.gitInfo());
  } catch {
    repos.SetGitInfo(null);
  }
}

/**
 * Resets the module between tests.
 */
export function ResetFetchSession(): void {
  generation = 0;
  activeId = null;
  pending = null;
}

/**
 * Applies one fetch event, if it belongs to the pass currently being accepted.
 *
 * @param mine - The generation captured when this handler was created.
 * @param event - What Rust sent.
 */
function HandleFetchEvent(mine: number, event: FetchEvent): void {
  if (mine !== generation) return;

  const repos = useReposStore();

  switch (event.kind) {
    case 'queued':
      // Rust's own list, which can be shorter than the one asked for. Anything queued optimistically
      // and not confirmed here is dropped, so nothing is left claiming work that will not happen.
      for (const path of repos.fetchingPaths) {
        if (!event.paths.includes(path)) repos.SetFetchState(path, null);
      }
      for (const path of event.paths) repos.SetFetchState(path, 'queued');
      break;

    case 'fetching':
      for (const path of event.paths) repos.SetFetchState(path, 'running');
      break;

    case 'results':
      for (const result of event.results) Settle(result);
      break;

    case 'finished':
      // Discharges every `queued` claim, including rows a cancellation meant never ran. A row left
      // on it would say work is coming for the rest of the session.
      repos.ClearFetchStates();
      activeId = null;
      break;
  }
}

/**
 * Records one repository's outcome.
 *
 * The row itself is not touched: Rust pushes the refreshed row on the session channel, and this is
 * only the per-path state beside it.
 *
 * @param result - What happened to one repository.
 */
function Settle(result: FetchOutcome): void {
  const repos = useReposStore();

  repos.SetFetchState(result.path, null);

  const message = failureMessage(result.status, result.detail);
  repos.SetFetchError(result.path, message);
}

/**
 * How a failed outcome should read, or `null` when it was not a failure.
 *
 * Four of the nine statuses are not failures and must not be reported as such: a repository with
 * nothing to fetch from is an answered question, one skipped by the repeat guard or a cancellation
 * had nothing attempted, and `ok` succeeded. Turning any of them into a red row would be the
 * uncomputed-is-not-zero rule broken in a new place.
 *
 * `detail` is git's own words and leads whenever there are any, because the classification is
 * advisory and the message is what a user can act on.
 *
 * @param status - What Rust classified it as.
 * @param detail - Git's own last words, when a process ran.
 * @returns The message to show, or `null`.
 */
function failureMessage(status: FetchStatus, detail: string | null): string | null {
  const fallback = FAILURE_FALLBACK[status];
  if (fallback === null) return null;
  return detail ?? fallback;
}
