import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it } from 'vite-plus/test';
import { DEFAULT_UI_SETTINGS } from '@/scripts/settings';
import { useViewStore } from '@/stores/view';

describe('useViewStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it('starts with an unfiltered table in the default order', () => {
    const view = useViewStore();

    expect(view.settings).toEqual(DEFAULT_UI_SETTINGS);
    expect(view.filtering).toBe(false);
  });

  it('turns a chip on and off again', () => {
    const view = useViewStore();

    view.ToggleChip('dirty');
    expect(view.chips).toEqual(['dirty']);
    expect(view.filtering).toBe(true);

    view.ToggleChip('dirty');
    expect(view.chips).toEqual([]);
    expect(view.filtering).toBe(false);
  });

  /**
   * A header click on the active column reverses it; on a new column it starts ascending. Carrying
   * the previous column's direction over would make one click do two things, which reads as a bug.
   */
  it('reverses the active column and starts a new one ascending', () => {
    const view = useViewStore();

    view.SortBy('read');
    expect([view.sortKey, view.sortDirection]).toEqual(['read', 'asc']);

    view.SortBy('read');
    expect([view.sortKey, view.sortDirection]).toEqual(['read', 'desc']);

    view.SortBy('stash');
    expect([view.sortKey, view.sortDirection]).toEqual(['stash', 'asc']);
  });

  /**
   * A query narrows the table exactly as a chip does, so it counts as filtering — which is what
   * makes an empty table explicable rather than alarming.
   */
  it('counts a query as filtering, and ignores whitespace', () => {
    const view = useViewStore();

    view.SetQuery('   ');
    expect(view.filtering).toBe(false);

    view.SetQuery('alpha');
    expect(view.filtering).toBe(true);
  });

  it('clears every filter without disturbing the sort', () => {
    const view = useViewStore();
    view.ToggleChip('dirty');
    view.SetQuery('alpha');
    view.SortBy('read');

    view.ClearFilters();

    expect(view.chips).toEqual([]);
    expect(view.query).toBe('');
    expect(view.sortKey).toBe('read');
  });

  /**
   * The query is deliberately not part of what gets persisted: a restored query would paint an
   * empty table at launch and make it look as though the repositories were gone.
   */
  it('leaves the query out of what is persisted', () => {
    const view = useViewStore();
    view.SetQuery('alpha');

    expect(Object.keys(view.settings)).not.toContain('query');
  });

  it('applies a saved view wholesale', () => {
    const view = useViewStore();

    view.Apply({
      chips: ['conflicted'],
      sortKey: 'worktree',
      sortDirection: 'desc',
      groupByFolder: true,
    });

    expect(view.chips).toEqual(['conflicted']);
    expect(view.sortKey).toBe('worktree');
    expect(view.sortDirection).toBe('desc');
    expect(view.groupByFolder).toBe(true);
  });

  /**
   * `settings` is what gets written to disk, so it must be a copy: handing out the store's own
   * array would let whatever holds it mutate the live view.
   */
  it('hands out a copy of its chips, not the live array', () => {
    const view = useViewStore();
    view.ToggleChip('dirty');

    view.settings.chips.push('detached');

    expect(view.chips).toEqual(['dirty']);
  });
});
