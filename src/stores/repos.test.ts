import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it } from 'vite-plus/test';
import { isRead, useReposStore } from '@/stores/repos';
import { MakeDiscovered, MakeStatus, MakeTotals } from '@/tests/fixtures';

describe('repos store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  describe('isRead', () => {
    it('narrows both ways on the structural difference alone', () => {
      expect(isRead(MakeStatus())).toBe(true);
      expect(isRead(MakeDiscovered())).toBe(false);
    });
  });

  it('replaces a discovered row with its status at the same key, keeping its position', () => {
    const store = useReposStore();
    store.UpsertMany([
      MakeDiscovered({ path: 'C:/work/a', name: 'a' }),
      MakeDiscovered({ path: 'C:/work/b', name: 'b' }),
    ]);

    store.Upsert(MakeStatus({ path: 'C:/work/a', name: 'a' }));

    expect(store.count).toBe(2);
    expect(store.readCount).toBe(1);
    expect([...store.byPath.keys()]).toEqual(['C:/work/a', 'C:/work/b']);
  });

  /**
   * `0` would render a full progress bar for a scan that has not started counting.
   */
  it('reports an unknown total as null rather than zero', () => {
    const store = useReposStore();

    expect(store.progress.total).toBeNull();

    store.SetDiscoverySummary({
      reposFound: 12,
      dirsVisited: 40,
      dirsPruned: 0,
      errors: [],
      elapsedMs: 5,
    });

    expect(store.progress.total).toBe(12);
  });

  /**
   * "Tier 0 has not finished" and "a repository failed" are separate questions, and the second can
   * be answered first.
   *
   * One field cannot carry both. `RepoErrors` arrives per batch, so a failure is known while the
   * scan runs — and a single nullable map whose `null` also meant "still running" could not express
   * that. The accumulating map and the done flag are distinct for that reason, and this test pins
   * the pair.
   */
  it('records a read failure before Tier 0 has finished', () => {
    const store = useReposStore();

    expect(store.tier0Done).toBe(false);
    expect(store.repoErrors.size).toBe(0);

    store.AddRepoErrors([{ path: 'C:/work/broken', message: 'HEAD is corrupt' }]);

    expect(store.repoErrors.get('C:/work/broken')).toBe('HEAD is corrupt');
    expect(store.tier0Done).toBe(false);
  });

  it('reports Tier 0 as done only once the totals arrive', () => {
    const store = useReposStore();

    expect(store.tier0Done).toBe(false);

    store.SetTotals(MakeTotals());

    expect(store.tier0Done).toBe(true);
  });

  /**
   * The terminal event repeats every failure already delivered per batch, so re-recording one must
   * overwrite rather than duplicate.
   */
  it('re-recording the same failure does not duplicate it', () => {
    const store = useReposStore();
    const failure = { path: 'C:/work/broken', message: 'HEAD is corrupt' };

    store.AddRepoErrors([failure]);
    store.AddRepoErrors([failure]);

    expect(store.repoErrors.size).toBe(1);
  });

  it('clears rows, summaries, and read failures on reset', () => {
    const store = useReposStore();
    store.Upsert(MakeStatus());
    store.SetTotals(MakeTotals());
    store.AddRepoErrors([{ path: 'C:/work/broken', message: 'boom' }]);
    store.SetDetailError('C:/work/alpha', 'counts failed');
    store.SetScanError('boom');

    store.Reset();

    expect(store.count).toBe(0);
    expect(store.totals).toBeNull();
    expect(store.tier0Done).toBe(false);
    expect(store.repoErrors.size).toBe(0);
    expect(store.detailErrors.size).toBe(0);
    expect(store.scanError).toBeNull();
  });

  /**
   * A rescan of the same tree must not collapse the drawers the user has open: expansion describes
   * what they are looking at, not what the previous scan found.
   */
  it('keeps expanded rows across a reset', () => {
    const store = useReposStore();
    store.ToggleExpanded('C:/work/alpha');

    store.Reset();

    expect(store.expanded.has('C:/work/alpha')).toBe(true);
  });

  describe('detail state', () => {
    it('toggles a row open and closed, reporting which it did', () => {
      const store = useReposStore();

      expect(store.ToggleExpanded('C:/a')).toBe(true);
      expect(store.expanded.has('C:/a')).toBe(true);
      expect(store.ToggleExpanded('C:/a')).toBe(false);
      expect(store.expanded.has('C:/a')).toBe(false);
    });

    it('tracks an in-flight read per row', () => {
      const store = useReposStore();

      store.SetLoadingDetail('C:/a', true);
      expect(store.loadingDetail.has('C:/a')).toBe(true);

      store.SetLoadingDetail('C:/a', false);
      expect(store.loadingDetail.has('C:/a')).toBe(false);
    });

    it('clears a detail failure when passed null', () => {
      const store = useReposStore();
      store.SetDetailError('C:/a', 'counts failed');

      store.SetDetailError('C:/a', null);

      expect(store.detailErrors.has('C:/a')).toBe(false);
    });
  });

  it('drops removed rows by path', () => {
    const store = useReposStore();
    store.UpsertMany([MakeStatus({ path: 'C:/a' }), MakeStatus({ path: 'C:/b' })]);

    store.Remove(['C:/a']);

    expect([...store.byPath.keys()]).toEqual(['C:/b']);
  });

  it('orders rows by folder then name', () => {
    const store = useReposStore();
    store.UpsertMany([
      MakeStatus({ path: 'C:/z/b', parent: 'C:/z', name: 'b' }),
      MakeStatus({ path: 'C:/a/y', parent: 'C:/a', name: 'y' }),
      MakeStatus({ path: 'C:/a/x', parent: 'C:/a', name: 'x' }),
    ]);

    expect(store.rows.map((row) => row.path)).toEqual(['C:/a/x', 'C:/a/y', 'C:/z/b']);
  });
});
