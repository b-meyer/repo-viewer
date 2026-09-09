import { describe, expect, it } from 'vite-plus/test';
import { DEFAULT_UI_SETTINGS, type FilterChip, type UiSettings } from '@/scripts/settings';
import { STALE_FETCH_MS } from '@/scripts/utils';
import { buildView } from '@/scripts/view';
import type { RepoRow } from '@/stores/repos';
import { MakeDiscovered, MakeStatus } from '@/tests/fixtures';

const NOW = 1_700_000_000_000;

/**
 * View options with only what a test cares about spelled out.
 */
function options(overrides: Partial<UiSettings> = {}): UiSettings {
  return { ...DEFAULT_UI_SETTINGS, ...overrides };
}

/**
 * The view for `rows` under `chips`, with no search.
 */
function withChips(rows: RepoRow[], chips: FilterChip[]) {
  return buildView(rows, options({ chips }), NOW, null);
}

/**
 * Every visible path, in order.
 */
function paths(view: ReturnType<typeof buildView>): string[] {
  return view.groups.flatMap((group) => group.rows.map((row) => row.path));
}

describe('buildView — filtering', () => {
  it('shows every row when no chip is active', () => {
    const view = withChips([MakeStatus({ path: 'C:/a' }), MakeDiscovered({ path: 'C:/b' })], []);

    expect(view.visible).toBe(2);
    expect(view.hidden).toBe(0);
    expect(view.pending).toBe(0);
  });

  it('keeps a row a chip matches and drops one it does not', () => {
    const view = withChips(
      [
        MakeStatus({ path: 'C:/dirty', dirty: true }),
        MakeStatus({ path: 'C:/clean', dirty: false }),
      ],
      ['dirty'],
    );

    expect(paths(view)).toEqual(['C:/dirty']);
    expect(view.hidden).toBe(1);
  });

  /**
   * **The rule this module exists for.** A row whose Tier 1 has not run is not clean — it is
   * unknown. Excluding it silently would report it as clean, which is the
   * uncomputed-renders-as-zero bug in filter form. It is excluded _and counted_, and the count is
   * what the toolbar shows.
   */
  it('counts a row whose tier has not run rather than treating it as a miss', () => {
    const view = withChips(
      [
        MakeStatus({ path: 'C:/dirty', dirty: true }),
        MakeStatus({ path: 'C:/unknown', dirty: null }),
        MakeStatus({ path: 'C:/clean', dirty: false }),
      ],
      ['dirty'],
    );

    expect(paths(view)).toEqual(['C:/dirty']);
    expect(view.pending).toBe(1);
    expect(view.hidden).toBe(2);
  });

  /**
   * The other half of that rule, and the distinction `AppUnknown` draws between `pending` and `na`:
   * a bare repository has no worktree, so its `dirty` is not "not yet" — it is "never".
   */
  it('does not call a bare repository pending, because it can never be dirty', () => {
    const view = withChips([MakeStatus({ path: 'C:/bare', kind: 'bare', dirty: null })], ['dirty']);

    expect(view.visible).toBe(0);
    expect(view.pending).toBe(0);
  });

  /**
   * A row Tier 0 never produced is the row most worth looking at, so no chip may hide it. It has no
   * field to judge, and a filter is not the place to decide it does not matter.
   */
  it('never hides a row that has no status at all', () => {
    const view = withChips(
      [MakeDiscovered({ path: 'C:/broken' }), MakeStatus({ path: 'C:/clean', dirty: false })],
      ['dirty'],
    );

    expect(paths(view)).toEqual(['C:/broken']);
    expect(view.pending).toBe(0);
  });

  it('unions its chips rather than intersecting them', () => {
    const view = withChips(
      [
        MakeStatus({ path: 'C:/dirty', dirty: true }),
        MakeStatus({ path: 'C:/detached', dirty: false, head: { kind: 'detached', id: 'a' } }),
        MakeStatus({ path: 'C:/neither', dirty: false }),
      ],
      ['dirty', 'detached'],
    );

    expect(paths(view).toSorted()).toEqual(['C:/detached', 'C:/dirty']);
  });

  /**
   * `ahead` is a Tier 0 field, so by the time there is a row to judge it has been attempted. `null`
   * means "no upstream to be ahead of", which is an answer and not a pending read.
   */
  it('treats a missing ahead count as an answer, not as pending', () => {
    const view = withChips(
      [
        MakeStatus({ path: 'C:/ahead', ahead: 2 }),
        MakeStatus({ path: 'C:/no-upstream', ahead: null }),
        MakeStatus({ path: 'C:/level', ahead: 0 }),
      ],
      ['unpushed'],
    );

    expect(paths(view)).toEqual(['C:/ahead']);
    expect(view.pending).toBe(0);
  });

  /**
   * Never fetched is the state this chip most needs to catch: ahead and behind are measured against
   * a remote ref that has never been updated, which is the most misleading pair the app can show.
   */
  it('counts a repository that has never been fetched as stale', () => {
    const view = withChips(
      [
        MakeStatus({ path: 'C:/never', lastFetchedMs: null }),
        MakeStatus({ path: 'C:/fresh', lastFetchedMs: NOW - 1000 }),
        MakeStatus({ path: 'C:/old', lastFetchedMs: NOW - STALE_FETCH_MS - 1000 }),
      ],
      ['staleFetch'],
    );

    expect(paths(view).toSorted()).toEqual(['C:/never', 'C:/old']);
  });

  it('narrows to the search matches when there is a query', () => {
    const view = buildView(
      [MakeStatus({ path: 'C:/a' }), MakeStatus({ path: 'C:/b' })],
      options(),
      NOW,
      new Set(['C:/b']),
    );

    expect(paths(view)).toEqual(['C:/b']);
    expect(view.hidden).toBe(1);
  });
});

describe('buildView — sorting', () => {
  it('orders by name in both directions', () => {
    const rows = [
      MakeStatus({ path: 'C:/b', name: 'beta' }),
      MakeStatus({ path: 'C:/a', name: 'alpha' }),
    ];

    expect(paths(buildView(rows, options({ sortKey: 'name' }), NOW, null))).toEqual([
      'C:/a',
      'C:/b',
    ]);
    expect(
      paths(buildView(rows, options({ sortKey: 'name', sortDirection: 'desc' }), NOW, null)),
    ).toEqual(['C:/b', 'C:/a']);
  });

  /**
   * **Unknown is last whichever way the arrow points.** Reversing a sort must not promote every
   * uncomputed value to the top: `null` is not a small number, it is no answer. This is the
   * ordering form of never rendering an uncomputed value as `0`, and the assertion that fails if
   * `null` is ever compared as one.
   */
  it('sorts rows with no value last in both directions', () => {
    const rows = [
      MakeStatus({ path: 'C:/none', name: 'none', ahead: null }),
      MakeStatus({ path: 'C:/two', name: 'two', ahead: 2 }),
      MakeStatus({ path: 'C:/one', name: 'one', ahead: 1 }),
    ];

    expect(paths(buildView(rows, options({ sortKey: 'sync' }), NOW, null))).toEqual([
      'C:/one',
      'C:/two',
      'C:/none',
    ]);
    expect(
      paths(buildView(rows, options({ sortKey: 'sync', sortDirection: 'desc' }), NOW, null)),
    ).toEqual(['C:/two', 'C:/one', 'C:/none']);
  });

  /**
   * A row with no status has none of the sortable fields, so it sorts last under every column but
   * the name — which is the same thing the table says about it in every other cell.
   */
  it('sorts an unread row last under a column it cannot answer', () => {
    const rows = [
      MakeDiscovered({ path: 'C:/unread', name: 'aaa' }),
      MakeStatus({ path: 'C:/read', name: 'zzz', stashCount: 1 }),
    ];

    expect(paths(buildView(rows, options({ sortKey: 'stash' }), NOW, null))).toEqual([
      'C:/read',
      'C:/unread',
    ]);
  });

  /**
   * Two rows with nothing to compare must not shuffle as a scan streams them in, or the table
   * reorders itself under the cursor.
   */
  it('breaks a tie by folder and name, so the order is stable', () => {
    const rows = [
      MakeStatus({ path: 'C:/w/b', parent: 'C:/w', name: 'b', ahead: null }),
      MakeStatus({ path: 'C:/w/a', parent: 'C:/w', name: 'a', ahead: null }),
    ];

    expect(paths(buildView(rows, options({ sortKey: 'sync' }), NOW, null))).toEqual([
      'C:/w/a',
      'C:/w/b',
    ]);
  });
});

describe('buildView — grouping', () => {
  it('returns one keyless group when grouping is off', () => {
    const view = buildView([MakeStatus({ path: 'C:/a' })], options(), NOW, null);

    expect(view.groups).toHaveLength(1);
    expect(view.groups[0]?.key).toBe('');
  });

  it('splits rows by folder, in folder order', () => {
    const view = buildView(
      [
        MakeStatus({ path: 'C:/work/a', parent: 'C:/work', name: 'a' }),
        MakeStatus({ path: 'C:/other/b', parent: 'C:/other', name: 'b' }),
        MakeStatus({ path: 'C:/work/c', parent: 'C:/work', name: 'c' }),
      ],
      options({ groupByFolder: true }),
      NOW,
      null,
    );

    expect(view.groups.map((group) => group.key)).toEqual(['C:/other', 'C:/work']);
    expect(view.groups[1]?.rows.map((row) => row.name)).toEqual(['a', 'c']);
  });

  /**
   * Grouping narrows the ordering rather than replacing it: the blocks are in folder order and the
   * rows inside each one keep the column sort.
   */
  it('keeps the column sort inside each group', () => {
    const view = buildView(
      [
        MakeStatus({ path: 'C:/w/a', parent: 'C:/w', name: 'a', stashCount: 1 }),
        MakeStatus({ path: 'C:/w/b', parent: 'C:/w', name: 'b', stashCount: 9 }),
      ],
      options({ groupByFolder: true, sortKey: 'stash', sortDirection: 'desc' }),
      NOW,
      null,
    );

    expect(view.groups[0]?.rows.map((row) => row.name)).toEqual(['b', 'a']);
  });
});
