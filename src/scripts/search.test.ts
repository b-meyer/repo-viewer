import { beforeEach, describe, expect, it } from 'vite-plus/test';
import {
  ClearIndex,
  IndexRows,
  indexVersion,
  MIN_QUERY_LENGTH,
  searchPaths,
  UnindexPaths,
} from '@/scripts/search';
import { MakeDiscovered, MakeStatus } from '@/tests/fixtures';

/**
 * Searches at the current corpus version, which is what a `computed` in a component does.
 */
function find(query: string): string[] {
  return [...(searchPaths(query, indexVersion.value) ?? [])].toSorted();
}

describe('search', () => {
  beforeEach(() => {
    // Module state outlives a test, so each one starts from an empty index.
    ClearIndex();
  });

  it('finds a repository by its name', () => {
    IndexRows([
      MakeStatus({ path: 'C:/work/dashboard', name: 'dashboard' }),
      MakeStatus({ path: 'C:/work/engine', name: 'engine' }),
    ]);

    expect(find('dashboard')).toEqual(['C:/work/dashboard']);
  });

  it('finds a repository by a fragment of its path', () => {
    IndexRows([
      MakeStatus({ path: 'C:/clients/acme/site', name: 'site' }),
      MakeStatus({ path: 'C:/work/engine', name: 'engine' }),
    ]);

    expect(find('acme')).toEqual(['C:/clients/acme/site']);
  });

  it('matches a prefix, so results appear while a name is still being typed', () => {
    IndexRows([MakeStatus({ path: 'C:/work/dashboard', name: 'dashboard' })]);

    expect(find('dash')).toEqual(['C:/work/dashboard']);
  });

  /**
   * Repository names are full of short fragments — `api`, `db`, `ui`, `ssg` — so anything under
   * five characters is matched exactly or not at all. Fuzziness there would make every short query
   * match half the tree.
   */
  it('is exact under five characters and forgiving above it', () => {
    IndexRows([
      MakeStatus({ path: 'C:/work/api', name: 'api' }),
      MakeStatus({ path: 'C:/work/dashboard', name: 'dashboard' }),
    ]);

    expect(find('apu')).toEqual([]);
    expect(find('dashbaord')).toEqual(['C:/work/dashboard']);
  });

  /**
   * Two words mean both, which is what a user typing them expects — but showing nothing at all is
   * worse than showing what one of them matched, so the OR pass is the fallback rather than the
   * default.
   */
  it('requires every term, then falls back to any of them', () => {
    IndexRows([
      MakeStatus({ path: 'C:/work/alpha', name: 'alpha', head: { kind: 'branch', name: 'main' } }),
      MakeStatus({ path: 'C:/work/beta', name: 'beta', head: { kind: 'branch', name: 'release' } }),
    ]);

    expect(find('alpha main')).toEqual(['C:/work/alpha']);
    expect(find('alpha release')).toEqual(['C:/work/alpha', 'C:/work/beta']);
  });

  /**
   * `null` and not an empty set. "No query" means show everything; "no matches" means show nothing,
   * and returning the second for the first would blank the table on the first keystroke.
   */
  it('answers null for a query too short to run', () => {
    IndexRows([MakeStatus({ path: 'C:/work/alpha', name: 'alpha' })]);

    expect(searchPaths('a'.repeat(MIN_QUERY_LENGTH - 1), indexVersion.value)).toBeNull();
    expect(searchPaths('   ', indexVersion.value)).toBeNull();
    expect(searchPaths('zzzzz', indexVersion.value)).toEqual(new Set());
  });

  /**
   * The corpus is live. A row arrives from discovery with a name and a path, then Tier 0 replaces
   * it with one that has a branch — and indexing the same id twice must update it rather than throw
   * or duplicate it.
   */
  it('replaces a row it already holds instead of duplicating it', () => {
    IndexRows([MakeDiscovered({ path: 'C:/work/alpha', name: 'alpha' })]);
    expect(find('alpha')).toEqual(['C:/work/alpha']);
    expect(find('feature')).toEqual([]);

    IndexRows([
      MakeStatus({
        path: 'C:/work/alpha',
        name: 'alpha',
        head: { kind: 'branch', name: 'feature' },
      }),
    ]);

    expect(find('alpha')).toEqual(['C:/work/alpha']);
    expect(find('feature')).toEqual(['C:/work/alpha']);
  });

  /**
   * A removed root and a repository a reconciling scan found to be gone both arrive as removals. A
   * search that kept offering them would hand the user a row the table no longer has.
   */
  it('forgets a row that was removed', () => {
    IndexRows([MakeStatus({ path: 'C:/work/alpha', name: 'alpha' })]);

    UnindexPaths(['C:/work/alpha']);

    expect(find('alpha')).toEqual([]);
  });

  it('forgets everything when a scan starts over', () => {
    IndexRows([MakeStatus({ path: 'C:/work/alpha', name: 'alpha' })]);

    ClearIndex();

    expect(find('alpha')).toEqual([]);
  });

  /**
   * The index is a `shallowRef`, so mutating it notifies nothing. This counter is the only signal a
   * `computed` can depend on, so every mutation has to move it.
   */
  it('bumps its version on every change, since the index itself cannot notify', () => {
    const start = indexVersion.value;

    IndexRows([MakeStatus({ path: 'C:/work/alpha' })]);
    const afterAdd = indexVersion.value;
    UnindexPaths(['C:/work/alpha']);
    const afterRemove = indexVersion.value;
    ClearIndex();

    expect(afterAdd).toBeGreaterThan(start);
    expect(afterRemove).toBeGreaterThan(afterAdd);
    expect(indexVersion.value).toBeGreaterThan(afterRemove);
  });

  /**
   * Tier 2 is deliberately not indexed: its counts are lazy and mostly unknown, so indexing them
   * would mean reindexing on every expand for nothing searchable.
   */
  it('does not index the lazy tier', () => {
    IndexRows([
      MakeStatus({
        path: 'C:/work/alpha',
        name: 'alpha',
        submodules: [
          { name: 'vendored', path: 'vendor/thing', recordedId: 'a'.repeat(40), headId: null },
        ],
      }),
    ]);

    expect(find('vendored')).toEqual([]);
  });
});
