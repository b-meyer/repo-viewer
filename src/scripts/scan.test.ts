import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { createPinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { CancelScan, ResetScanSession, StartScan, StartSession } from '@/scripts/scan';
import { useReposStore } from '@/stores/repos';
import { MakeChannelDriver, ReadNumber } from '@/tests/channel';
import { MakeDiscovered, MakeStatus } from '@/tests/fixtures';

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
        summary: { reposRead: 3, errors: [], elapsedMs: 146 },
      });
      return 1;
    });

    await StartScan(['C:/work']);

    const store = useReposStore();
    expect(store.phase).toBe('done');
    expect(store.progress.total).toBe(3);
    expect(store.readErrors).not.toBeNull();
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
