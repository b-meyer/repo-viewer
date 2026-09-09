import type { RepoEvent } from '@/scripts/generated/RepoEvent';
import type { ScanEvent } from '@/scripts/generated/ScanEvent';
import type { ScanId } from '@/scripts/generated/ScanId';
/**
 * The scan session: which scan is current, and what its events do to the store.
 *
 * A module-scoped singleton rather than a composable or a store. There is exactly one scan session
 * per app, none of this state is rendered directly, and the module-singleton shape is already the
 * house pattern for a single shared instance.
 *
 * **This is the only file in `src/` that switches on an event's `kind`.** Everything above it sees
 * domain calls; everything below it sees rows.
 *
 * # Why a generation counter and not just the scan id
 *
 * The rule is that the frontend keeps the id of the scan it asked for and drops events from any
 * other. Taken literally that cannot be implemented: `scanRoots` resolves with the id _after_ Rust
 * may already have sent a batch, so there is a window in which events arrive and the id to compare
 * them against is not known yet. It is a real race, not a test artefact — Rust starts the pipeline
 * before the invoke's reply crosses back.
 *
 * So the **primary** filter is the generation counter, which is captured in the handler closure
 * before the invoke happens and therefore has no window at all. The id check is the guard on top:
 * `activeId` latches from the first accepted event, and once latched, an event bearing a different
 * id is dropped. The resolved id then cross-checks the latch rather than establishing it.
 */
import * as ipc from '@/scripts/ipc';
import { useReposStore } from '@/stores/repos';

/// Data
/**
 * Bumped by every scan start and every cancel. An event is accepted only if the generation captured
 * in its handler still matches, which is what makes a superseded scan's batches inert immediately.
 */
let generation = 0;

/**
 * The id of the scan whose events are being accepted, latched from its first event.
 */
let activeId: ScanId | null = null;

/**
 * The in-flight `scanRoots` call, so a cancel arriving before it resolves is not lost.
 */
let pending: Promise<ScanId> | null = null;

/**
 * Whether the session channel has been opened. Guards HMR's repeated mounts.
 */
let subscribed = false;

/**
 * Whether a cancel has already gone out for the current scan attempt.
 *
 * Two paths can want to cancel the same scan — an explicit {@link CancelScan}, and {@link StartScan}
 * discovering on resolve that it has been superseded — and which one gets there first depends on
 * when the invoke resolves. Rust treats a duplicate as a no-op, so this is tidiness rather than
 * correctness, but a command fired twice for one user action is worth not doing.
 */
let cancelSent = false;

/// Methods
/**
 * Opens the session channel and mirrors the rows Rust already holds.
 *
 * Idempotent: Vite HMR re-runs the mounting component's `onMounted`, and an unguarded call would
 * register a second channel that never goes away.
 */
export async function StartSession(): Promise<void> {
  if (subscribed) return;
  subscribed = true;

  const repos = useReposStore();
  try {
    const rows = await ipc.subscribe(HandleSessionEvent);
    repos.UpsertMany(rows);
  } catch (error) {
    subscribed = false;
    throw error;
  }
}

/**
 * Starts a scan over `roots`, replacing whatever scan was running.
 *
 * @param roots - The configured roots to walk.
 */
export async function StartScan(roots: string[]): Promise<void> {
  const repos = useReposStore();

  generation += 1;
  const mine = generation;
  activeId = null;
  cancelSent = false;

  // Before the invoke, so the old rows are gone before any new row can arrive.
  repos.Reset();
  repos.SetPhase('discovering');

  const request = ipc.scanRoots(roots, {}, (event) => {
    HandleScanEvent(mine, event);
  });
  pending = request;

  try {
    const id = await request;

    if (mine !== generation) {
      // A newer scan started, or a cancel landed, while this one was still being acknowledged.
      // Nothing from it is being accepted any more, so stop the work too.
      //
      // Rust's own `scan_roots` cancels every running scan before starting one, so the
      // newer-scan case is usually already handled server-side. It is not guaranteed to be: if
      // this invoke had not yet reached Rust when the newer one did, there was nothing there to
      // cancel, and this is what stops a superseded walk nobody is listening to.
      await SendCancel(id);
      return;
    }
    if (activeId !== null && activeId !== id) {
      // Events for a different scan arrived on our own channel. Rust would have to be wrong for
      // this to happen, so it is surfaced rather than quietly tolerated.
      repos.SetScanError(`scan ${String(id)} reported results for scan ${String(activeId)}`);
      repos.SetPhase('failed');
      return;
    }
    activeId = id;
  } catch (error) {
    if (mine !== generation) return;
    repos.SetScanError(error instanceof Error ? error.message : String(error));
    repos.SetPhase('failed');
  }
}

/**
 * Stops the current scan.
 *
 * Bumps the generation first, so no further event from the in-flight channel is accepted even if
 * the id is still unknown; the cancel itself is then queued behind the same promise, so a cancel
 * requested before the id resolves is not dropped.
 */
export async function CancelScan(): Promise<void> {
  const repos = useReposStore();
  if (!repos.scanning) return;

  generation += 1;
  repos.SetPhase('cancelled');

  const known = activeId;
  const request = pending;
  activeId = null;

  // An id already latched from an event can be cancelled straight away. One that is still unknown
  // has to wait for the invoke that will produce it — which is the case this whole dance exists
  // for, since it is precisely when the user cancels a scan that has barely started.
  if (known !== null) {
    await SendCancel(known);
    return;
  }
  if (request === null) return;

  try {
    await SendCancel(await request);
  } catch {
    // The scan never started, or had already finished. Neither is a failure: the phase is already
    // `cancelled`, which is what the user asked for.
  }
}

/**
 * Sends a cancel for `id`, at most once per scan attempt.
 *
 * @param id - The scan to stop.
 */
async function SendCancel(id: ScanId): Promise<void> {
  if (cancelSent) return;
  cancelSent = true;
  await ipc.cancelScan(id);
}

/**
 * Forgets the session and scan state.
 *
 * A test seam. Module state outlives a component, so without this each test would inherit the
 * previous one's generation and subscription.
 */
export function ResetScanSession(): void {
  generation = 0;
  activeId = null;
  pending = null;
  cancelSent = false;
  subscribed = false;
}

/**
 * Applies one scan event, if it belongs to the scan currently being accepted.
 *
 * @param mine - The generation the handler was created in.
 * @param event - The event Rust sent.
 */
function HandleScanEvent(mine: number, event: ScanEvent): void {
  if (mine !== generation) return;
  if (activeId !== null && event.scanId !== activeId) return;
  activeId = event.scanId;

  const repos = useReposStore();
  switch (event.kind) {
    case 'reposFound': {
      repos.UpsertMany(event.repos);
      break;
    }
    case 'reposUpdated': {
      repos.UpsertMany(event.repos);
      repos.SetPhase('reading');
      break;
    }
    case 'progress': {
      // Counts are rendered from the store's own rows; this variant exists so Rust remains the
      // authority on them, and it is where a future desync would show up first.
      break;
    }
    case 'discoveryFinished': {
      repos.SetDiscoverySummary(event.summary);
      repos.SetPhase('reading');
      break;
    }
    case 'finished': {
      repos.SetTier0Summary(event.summary);
      repos.SetPhase('done');
      break;
    }
    case 'cancelled': {
      repos.SetPhase('cancelled');
      break;
    }
  }
}

/**
 * Applies one session event.
 *
 * These carry no scan id: the session channel is always the current one, so there is nothing to
 * filter against.
 *
 * @param event - The event Rust sent.
 */
function HandleSessionEvent(event: RepoEvent): void {
  const repos = useReposStore();
  switch (event.kind) {
    case 'updated': {
      repos.UpsertMany(event.repos);
      break;
    }
    case 'removed': {
      repos.Remove(event.paths);
      break;
    }
  }
}
