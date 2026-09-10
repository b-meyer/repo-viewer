import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { createPinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { CancelFetch, LoadGitInfo, ResetFetchSession, StartFetch } from '@/scripts/fetch';
import type { FetchOutcome } from '@/scripts/generated/FetchOutcome';
import type { FetchStatus } from '@/scripts/generated/FetchStatus';
import { useReposStore } from '@/stores/repos';
import { MakeChannelDriver } from '@/tests/channel';
import { MakeStatus } from '@/tests/fixtures';

/**
 * One outcome, defaulting to a success.
 */
function outcome(path: string, status: FetchStatus = 'ok', detail: string | null = null) {
  return { path, status, detail, elapsedMs: 12 } satisfies FetchOutcome;
}

/**
 * An empty summary, for the terminal event.
 */
function summary(overrides: Record<string, unknown> = {}) {
  return {
    attempted: 0,
    succeeded: 0,
    failed: 0,
    skipped: 0,
    cancelled: false,
    elapsedMs: 5,
    ...overrides,
  };
}

/**
 * Lets the microtask queue drain, so an awaited invoke has resolved.
 */
async function Settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe('fetch session', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  afterEach(() => {
    clearMocks();
    ResetFetchSession();
  });

  /**
   * The production race, reproduced: Rust starts the pass before the invoke's reply crosses back,
   * so events can arrive while the id is still unknown. The generation counter is captured in the
   * handler closure before the invoke, which is what leaves no window at all.
   */
  it('keeps events that arrive before the fetch id resolves', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    // Built here rather than inside the handler, so the reply can be held open while events are
    // delivered — which is the whole shape of the race.
    let resolveId!: (id: number) => void;
    const reply = new Promise<number>((resolve) => {
      resolveId = resolve;
    });

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      // Everything lands before the reply.
      drive.Send(id, { kind: 'queued', paths: ['C:/a'] });
      drive.Send(id, { kind: 'fetching', paths: ['C:/a'] });
      return reply;
    });

    const started = StartFetch(['C:/a']);
    await Settle();

    expect(repos.fetchStates.get('C:/a')).toBe('running');

    resolveId(1);
    await started;
    expect(repos.fetchStates.get('C:/a')).toBe('running');
  });

  /**
   * "Queued" and "running" are different claims. With a concurrency cap a repository can wait
   * minutes before its process starts, so a spinner for the whole wait would say work is happening
   * when none is.
   */
  it('keeps a queued row distinct from a running one', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'queued', paths: ['C:/a', 'C:/b'] });
      drive.Send(id, { kind: 'fetching', paths: ['C:/a'] });
      return 1;
    });

    await StartFetch(['C:/a', 'C:/b']);

    expect(repos.fetchStates.get('C:/a')).toBe('running');
    expect(repos.fetchStates.get('C:/b')).toBe('queued');
  });

  /**
   * A row leaves the in-flight set when it **settles**, not when it starts. A count of starts would
   * let a progress bar read 100% with four fetches still running.
   */
  it('counts a repository once it has settled, not once it has started', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'queued', paths: ['C:/a', 'C:/b'] });
      drive.Send(id, { kind: 'fetching', paths: ['C:/a', 'C:/b'] });
      drive.Send(id, { kind: 'results', results: [outcome('C:/a')] });
      return 1;
    });

    await StartFetch(['C:/a', 'C:/b']);

    expect(repos.fetchStates.has('C:/a')).toBe(false);
    expect(repos.fetchStates.get('C:/b')).toBe('running');
  });

  /**
   * Four of the nine statuses are not failures, and reporting them as such would put a red row on a
   * repository that is perfectly fine — most often one with no remote at all, which is the
   * commonest non-success on a developer's tree.
   */
  it('does not report a repository with nothing to fetch from as a failure', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, {
        kind: 'results',
        results: [
          outcome('C:/a', 'noRemote'),
          outcome('C:/b', 'tooSoon'),
          outcome('C:/c', 'cancelled'),
          outcome('C:/d', 'ok'),
        ],
      });
      return 1;
    });

    await StartFetch(['C:/a', 'C:/b', 'C:/c', 'C:/d']);

    expect([...repos.fetchErrors.keys()]).toEqual([]);
  });

  /**
   * Git's own words lead, because the classification is advisory and the message is the part a user
   * can act on.
   */
  it('shows git own words for a failure rather than a category', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, {
        kind: 'results',
        results: [outcome('C:/a', 'auth', 'fatal: Authentication failed for https://x')],
      });
      return 1;
    });

    await StartFetch(['C:/a']);

    expect(repos.fetchErrors.get('C:/a')).toBe('fatal: Authentication failed for https://x');
  });

  /**
   * A failure the classifier recognised but git said nothing about still has to read as something.
   */
  it('falls back to a category when git said nothing', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'results', results: [outcome('C:/a', 'network', null)] });
      return 1;
    });

    await StartFetch(['C:/a']);

    expect(repos.fetchErrors.get('C:/a')).toBe('The remote could not be reached.');
  });

  /**
   * A fetch failure never lands on the row. Tier 0 owns `RepoStatus.error` and replaces it, so the
   * fetch's own Tier 0 re-read would erase the failure milliseconds after recording it.
   */
  it('keeps a fetch failure off the row error slot', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();
    repos.Upsert(MakeStatus({ path: 'C:/a' }));

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'results', results: [outcome('C:/a', 'failed', 'boom')] });
      return 1;
    });

    await StartFetch(['C:/a']);

    expect(repos.fetchErrors.get('C:/a')).toBe('boom');
    const row = repos.byPath.get('C:/a');
    expect(row && 'error' in row ? row.error : undefined).toBeNull();
  });

  /**
   * Each error map is owned by one operation and cleared by that operation's next attempt.
   */
  it('clears a previous failure when the row is fetched again', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();
    repos.SetFetchError('C:/a', 'the last one failed');

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'results', results: [outcome('C:/a')] });
      return 1;
    });

    await StartFetch(['C:/a']);

    expect(repos.fetchErrors.has('C:/a')).toBe(false);
  });

  /**
   * The terminal event discharges every `queued` claim, including for rows a cancellation meant
   * never ran. A row left on it would say work is coming for the rest of the session — which is the
   * frontend half of the suppression-leak bug.
   */
  it('clears every per-row state on the terminal event, including rows that never ran', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'queued', paths: ['C:/a', 'C:/b'] });
      drive.Send(id, { kind: 'results', results: [outcome('C:/a')] });
      drive.Send(id, { kind: 'finished', summary: summary({ cancelled: true }) });
      return 1;
    });

    await StartFetch(['C:/a', 'C:/b']);

    expect(repos.fetchStates.size).toBe(0);
    expect(repos.fetching).toBe(false);
  });

  /**
   * Failures survive the terminal event. They are the thing the user is meant to read afterwards.
   */
  it('keeps the failures after the pass ends', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'results', results: [outcome('C:/a', 'failed', 'boom')] });
      drive.Send(id, { kind: 'finished', summary: summary({ failed: 1 }) });
      return 1;
    });

    await StartFetch(['C:/a']);

    expect(repos.fetchErrors.get('C:/a')).toBe('boom');
  });

  /**
   * A refused command means nothing will ever arrive on the channel, so the optimistic queued state
   * has to be unwound here or every row spins for the rest of the session.
   */
  it('unwinds the queued state when the command is refused', async () => {
    const repos = useReposStore();

    mockIPC((command) => {
      if (command !== 'fetch_repos') return null;
      throw new Error('`C:/a` is not a known repository');
    });

    await StartFetch(['C:/a']);

    expect(repos.fetchStates.size).toBe(0);
    expect(repos.scanError).toContain('not a known repository');
  });

  /**
   * Rust's accepted list can be shorter than the one asked for. Anything queued optimistically and
   * not confirmed must not be left claiming work that will never happen.
   */
  it('drops a row Rust did not accept', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'queued', paths: ['C:/a'] });
      return 1;
    });

    await StartFetch(['C:/a', 'C:/b']);

    expect(repos.fetchStates.has('C:/a')).toBe(true);
    expect(repos.fetchStates.has('C:/b')).toBe(false);
  });

  /**
   * This module never mirrors a row. Rows arrive on the **session** channel, where `scan.ts`
   * applies them exactly as it applies a watcher push.
   */
  it('never writes a row', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();

    mockIPC((command, args) => {
      if (command !== 'fetch_repos') return null;
      const id = drive.Capture(args);
      drive.Send(id, { kind: 'queued', paths: ['C:/a'] });
      drive.Send(id, { kind: 'results', results: [outcome('C:/a')] });
      drive.Send(id, { kind: 'finished', summary: summary({ succeeded: 1 }) });
      return 1;
    });

    await StartFetch(['C:/a']);

    expect(repos.byPath.size).toBe(0);
  });

  /**
   * Cancelling clears the rows immediately rather than waiting for a terminal event that may never
   * arrive, and stops accepting anything still in flight.
   */
  it('clears the rows on cancel without waiting for the terminal event', async () => {
    const drive = MakeChannelDriver();
    const repos = useReposStore();
    const cancelled: number[] = [];

    mockIPC((command, args) => {
      if (command === 'fetch_repos') {
        const id = drive.Capture(args);
        drive.Send(id, { kind: 'queued', paths: ['C:/a'] });
        return 7;
      }
      if (command === 'cancel_fetch') {
        cancelled.push(1);
        return null;
      }
      return null;
    });

    await StartFetch(['C:/a']);
    await CancelFetch();

    expect(repos.fetchStates.size).toBe(0);
    expect(cancelled).toHaveLength(1);
  });

  /**
   * `undefined` is not `null`. Before the answer arrives the controls must not claim git is
   * missing, because that explanation might be false.
   */
  it('does not claim git is missing before it has asked', () => {
    const repos = useReposStore();

    expect(repos.gitInfo).toBeUndefined();
  });

  /**
   * A bridge failure is not evidence that git is absent, but reporting nothing found is the safe
   * answer: the controls disable and say so rather than failing at click time.
   */
  it('records what git was found', async () => {
    const repos = useReposStore();
    mockIPC((command) => {
      if (command !== 'git_info') return null;
      return { path: 'C:/Program Files/Git/bin/git.exe', version: 'git version 2.54.0' };
    });

    await LoadGitInfo();

    expect(repos.gitInfo?.version).toBe('git version 2.54.0');
  });
});
