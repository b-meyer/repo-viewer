import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { createPinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import {
  CancelScan,
  ReconcileOnLaunch,
  ResetScanSession,
  StartScan,
  StartSession,
} from '@/scripts/scan';
import { ClearIndex, indexVersion, searchPaths } from '@/scripts/search';
import { useReposStore } from '@/stores/repos';
import { MakeChannelDriver, ReadNumber } from '@/tests/channel';
import { MakeDiscovered, MakeStatus, MakeTotals } from '@/tests/fixtures';

/**
 * A `reposFound` batch for one path.
 */
function found(scanId: number, path: string) {
  return { kind: 'reposFound', scanId, repos: [MakeDiscovered({ path, name: path })] };
}

describe('scan session', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  afterEach(() => {
    clearMocks();
    ResetScanSession();
    ClearIndex();
  });

  /**
   * The production race, reproduced exactly: Rust sends a batch before the invoke's reply crosses
   * back, so the id is not known when the first event arrives.
   *
   * This is the test that fails without the latch, and the reason the generation counter — not the
   * scan id — is the primary filter.
   */
  it('keeps events that arrive before the scan id resolves', async () => {
    const drive = MakeChannelDriver();
    mockIPC((cmd, args) => {
      if (cmd !== 'scan_roots') return null;
      const id = drive.Capture(args);
      drive.Send(id, found(7, 'C:/work/a'));
      return 7;
    });

    await StartScan(['C:/work']);

    expect(useReposStore().count).toBe(1);
  });

  it('replaces a discovered row with its status at the same key', async () => {
    const drive = MakeChannelDriver();
    mockIPC((cmd, args) => {
      if (cmd !== 'scan_roots') return null;
      const id = drive.Capture(args);
      drive.Send(id, found(1, 'C:/work/a'));
      drive.Send(id, {
        kind: 'reposUpdated',
        scanId: 1,
        repos: [MakeStatus({ path: 'C:/work/a', name: 'C:/work/a', dirty: true })],
      });
      return 1;
    });

    await StartScan(['C:/work']);

    const store = useReposStore();
    expect(store.count).toBe(1);
    expect(store.readCount).toBe(1);
  });

  /**
   * Both drop mechanisms in one sequence: a batch on the _previous_ channel after a new scan has
   * started (the generation filter), and a batch carrying a foreign id on the current channel (the
   * id guard, which is the literal requirement).
   */
  it('drops events from a stale scan and from a foreign scan id', async () => {
    const drive = MakeChannelDriver();
    const ids: number[] = [];
    let next = 1;
    mockIPC((cmd, args) => {
      if (cmd === 'scan_roots') {
        ids.push(drive.Capture(args));
        return next++;
      }
      return null;
    });

    await StartScan(['C:/work']);
    drive.Send(ids[0], found(1, 'C:/work/a'));
    expect(useReposStore().count).toBe(1);

    await StartScan(['C:/other']);
    drive.Send(ids[0], found(1, 'C:/work/late')); // stale channel
    drive.Send(ids[1], found(2, 'C:/other/b')); // current
    drive.Send(ids[1], found(999, 'C:/other/ghost')); // foreign id

    expect([...useReposStore().byPath.keys()]).toEqual(['C:/other/b']);
  });

  it('records the summaries and ends in the done phase', async () => {
    const drive = MakeChannelDriver();
    mockIPC((cmd, args) => {
      if (cmd !== 'scan_roots') return null;
      const id = drive.Capture(args);
      drive.Send(id, {
        kind: 'discoveryFinished',
        scanId: 1,
        summary: { reposFound: 3, dirsVisited: 90, dirsPruned: 1, errors: [], elapsedMs: 58 },
      });
      drive.Send(id, {
        kind: 'finished',
        scanId: 1,
        summary: MakeTotals({ reposRead: 3 }),
      });
      return 1;
    });

    await StartScan(['C:/work']);

    const store = useReposStore();
    expect(store.phase).toBe('done');
    expect(store.progress.total).toBe(3);
    expect(store.tier0Done).toBe(true);
    expect(store.totals?.tier1Ms).toBe(700);
  });

  /**
   * A repository that will never produce a row is reported as its batch is read, so the row can
   * stop claiming to be "counting…" without waiting for the scan to end.
   */
  it('records a read failure from a mid-scan event', async () => {
    const drive = MakeChannelDriver();
    mockIPC((cmd, args) => {
      if (cmd !== 'scan_roots') return null;
      const id = drive.Capture(args);
      drive.Send(id, {
        kind: 'repoErrors',
        scanId: 1,
        errors: [{ path: 'C:/work/broken', message: 'HEAD is corrupt' }],
      });
      return 1;
    });

    await StartScan(['C:/work']);

    const store = useReposStore();
    expect(store.repoErrors.get('C:/work/broken')).toBe('HEAD is corrupt');
    // Still running: the failure arriving does not mean Tier 0 has finished, and conflating those
    // is what the old single-field contract got wrong.
    expect(store.tier0Done).toBe(false);
  });

  /**
   * The terminal event repeats every failure, so a webview that reloaded mid-scan catches up.
   */
  it('records the failures the terminal event repeats', async () => {
    const drive = MakeChannelDriver();
    mockIPC((cmd, args) => {
      if (cmd !== 'scan_roots') return null;
      const id = drive.Capture(args);
      drive.Send(id, {
        kind: 'finished',
        scanId: 1,
        summary: MakeTotals({
          reposRead: 0,
          errors: [{ path: 'C:/work/broken', message: 'HEAD is corrupt' }],
        }),
      });
      return 1;
    });

    await StartScan(['C:/work']);

    expect(useReposStore().repoErrors.get('C:/work/broken')).toBe('HEAD is corrupt');
  });

  it('surfaces a failed scan command rather than leaving the spinner running', async () => {
    mockIPC(() => {
      throw new Error('not a configured root');
    });

    await StartScan(['C:/nope']);

    const store = useReposStore();
    expect(store.phase).toBe('failed');
    expect(store.scanError).toContain('not a configured root');
  });

  /**
   * A cancel requested before the id resolves must still reach Rust.
   */
  it('cancels a scan whose id has not resolved yet', async () => {
    const drive = MakeChannelDriver();
    const cancelled: number[] = [];
    // Held open so the cancel is requested while the id is genuinely still unknown, which is the
    // whole point of the test.
    let release!: (value: number) => void;
    const held = new Promise<number>((resolve) => {
      release = resolve;
    });

    mockIPC((cmd, args) => {
      if (cmd === 'scan_roots') {
        drive.Capture(args);
        return held;
      }
      if (cmd === 'cancel_scan') {
        cancelled.push(ReadNumber(args, 'id'));
      }
      return null;
    });

    const scan = StartScan(['C:/work']);
    const cancel = CancelScan();
    release(42);
    await Promise.all([scan, cancel]);

    expect(cancelled).toEqual([42]);
    expect(useReposStore().phase).toBe('cancelled');
  });

  it('keeps the rows already read when a scan is cancelled', async () => {
    const drive = MakeChannelDriver();
    mockIPC((cmd, args) => {
      if (cmd !== 'scan_roots') return null;
      const id = drive.Capture(args);
      drive.Send(id, found(1, 'C:/work/a'));
      drive.Send(id, { kind: 'cancelled', scanId: 1, found: 1, read: 0 });
      return 1;
    });

    await StartScan(['C:/work']);

    const store = useReposStore();
    expect(store.phase).toBe('cancelled');
    expect(store.count).toBe(1);
  });

  /**
   * The two kinds of scan, and the difference that matters. Pressing Scan starts over, so the rows
   * go before the first new one can arrive.
   */
  it('drops the existing rows when a scan starts over', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/stale' }));

    mockIPC((cmd) => (cmd === 'scan_roots' ? 1 : null));
    await StartScan(['C:/work']);

    expect(store.count).toBe(0);
  });

  /**
   * **A reconcile must not.** The rows on screen at launch are the ones Rust restored from the
   * cache, and clearing them would make the window flash empty for the length of a scan — worse
   * than never having cached them. The stream overwrites each row as it is re-read, and Rust evicts
   * whatever the walk did not find.
   */
  it('keeps the cached rows when a launch reconcile starts', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/cached' }));

    mockIPC((cmd) => (cmd === 'scan_roots' ? 1 : null));
    await ReconcileOnLaunch(['C:/work']);

    expect(store.count).toBe(1);
  });

  /**
   * Vite HMR re-runs the mounting component's `onMounted`, and a reconcile per reload would rescan
   * the tree every time a file is saved.
   */
  it('reconciles only once, however often it is asked', async () => {
    let scans = 0;
    mockIPC((cmd) => {
      if (cmd !== 'scan_roots') return null;
      scans += 1;
      return scans;
    });

    await ReconcileOnLaunch(['C:/work']);
    await ReconcileOnLaunch(['C:/work']);

    expect(scans).toBe(1);
  });

  it('does not reconcile when there are no roots to walk', async () => {
    let scans = 0;
    mockIPC((cmd) => {
      if (cmd !== 'scan_roots') return null;
      scans += 1;
      return 1;
    });

    await ReconcileOnLaunch([]);

    expect(scans).toBe(0);
  });

  /**
   * This module is the seam where events become store writes, so it is also where the search index
   * is kept in step. A row found by the walk is searchable before its refs have been read.
   */
  it('indexes rows as they arrive, before any tier has read them', async () => {
    const drive = MakeChannelDriver();
    mockIPC((cmd, args) => {
      if (cmd !== 'scan_roots') return null;
      const id = drive.Capture(args);
      drive.Send(id, found(1, 'C:/work/alpha'));
      return 1;
    });

    await StartScan(['C:/work']);

    expect(searchPaths('alpha', indexVersion.value)).toEqual(new Set(['C:/work/alpha']));
  });

  describe('session channel', () => {
    it('mirrors the snapshot Rust returns', async () => {
      mockIPC((cmd) => (cmd === 'subscribe' ? [MakeStatus({ path: 'C:/work/a' })] : null));

      await StartSession();

      expect(useReposStore().count).toBe(1);
    });

    it('drops rows Rust removed, with no scan running', async () => {
      const drive = MakeChannelDriver();
      let channelId = 0;
      mockIPC((cmd, args) => {
        if (cmd !== 'subscribe') return null;
        channelId = drive.Capture(args);
        return [MakeStatus({ path: 'C:/work/a' }), MakeStatus({ path: 'C:/work/b' })];
      });

      await StartSession();
      expect(useReposStore().count).toBe(2);

      drive.Send(channelId, { kind: 'removed', paths: ['C:/work/a'] });

      expect([...useReposStore().byPath.keys()]).toEqual(['C:/work/b']);
    });

    /**
     * A row Rust evicted — a removed root, or a repository a reconciling scan found to be gone —
     * has to leave the index too, or a search keeps offering a row the table no longer has.
     */
    it('unindexes rows Rust removed', async () => {
      const drive = MakeChannelDriver();
      let channelId = 0;
      mockIPC((cmd, args) => {
        if (cmd !== 'subscribe') return null;
        channelId = drive.Capture(args);
        return [MakeStatus({ path: 'C:/work/alpha', name: 'alpha' })];
      });

      await StartSession();
      expect(searchPaths('alpha', indexVersion.value)).toEqual(new Set(['C:/work/alpha']));

      drive.Send(channelId, { kind: 'removed', paths: ['C:/work/alpha'] });

      expect(searchPaths('alpha', indexVersion.value)).toEqual(new Set());
    });

    it('subscribes only once, so HMR remounts do not stack channels', async () => {
      let calls = 0;
      mockIPC((cmd) => {
        if (cmd === 'subscribe') calls += 1;
        return [];
      });

      await StartSession();
      await StartSession();

      expect(calls).toBe(1);
    });
  });
});
