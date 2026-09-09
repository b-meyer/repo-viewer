import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it } from 'vite-plus/test';
import { isRead, useReposStore } from '@/stores/repos';
import { MakeDiscovered, MakeStatus } from '@/tests/fixtures';

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
   * "Tier 0 has not finished" and "finished with nothing wrong" are different facts.
   */
  it('reports read errors as null until Tier 0 finishes', () => {
    const store = useReposStore();

    expect(store.readErrors).toBeNull();

    store.SetTier0Summary({ reposRead: 3, errors: [], elapsedMs: 9 });

    expect(store.readErrors).not.toBeNull();
    expect(store.readErrors?.size).toBe(0);
  });

  it('clears rows and summaries on reset', () => {
    const store = useReposStore();
    store.Upsert(MakeStatus());
    store.SetTier0Summary({ reposRead: 1, errors: [], elapsedMs: 1 });
    store.SetScanError('boom');

    store.Reset();

    expect(store.count).toBe(0);
    expect(store.tier0).toBeNull();
    expect(store.scanError).toBeNull();
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
