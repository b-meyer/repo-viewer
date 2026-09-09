import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import { FILTER_CHIPS, type FilterChip } from '@/scripts/settings';
import FilterBar from './FilterBar.vue';

function mountBar(
  extra: {
    chips?: FilterChip[];
    query?: string;
    groupByFolder?: boolean;
    counts?: { visible: number; hidden: number; pending: number };
    total?: number;
    disabled?: boolean;
  } = {},
) {
  return mount(FilterBar, {
    props: {
      chips: extra.chips ?? [],
      query: extra.query ?? '',
      groupByFolder: extra.groupByFolder ?? false,
      counts: extra.counts ?? { visible: 0, hidden: 0, pending: 0 },
      total: extra.total ?? 0,
      disabled: extra.disabled ?? false,
    },
  });
}

/**
 * The toggle whose label contains `label`.
 */
function toggle(wrapper: ReturnType<typeof mountBar>, label: string) {
  const found = wrapper.findAll('button').find((button) => button.text().includes(label));
  if (found === undefined) throw new Error(`no control labelled ${label}`);
  return found;
}

describe('FilterBar', () => {
  /**
   * One toggle per chip, plus the grouping one. Counted against `FILTER_CHIPS` rather than against
   * a literal, so adding a chip to the type and forgetting the bar fails here.
   */
  it('offers one toggle per chip, plus grouping', () => {
    const wrapper = mountBar();

    expect(wrapper.findAll('[aria-pressed]')).toHaveLength(FILTER_CHIPS.length + 1);
    expect(wrapper.text()).toContain('Group by folder');
  });

  /**
   * Two of the chips need a qualification a label cannot carry: "unpushed" is relative to the last
   * fetch, and "stale fetch" includes a repository that has never been fetched at all. Both live in
   * the tooltip, and a chip without one is a rule the user cannot find out about.
   */
  it('explains each chip rule in its tooltip', () => {
    const wrapper = mountBar();
    const titled = wrapper
      .findAll('[aria-pressed]')
      .filter((control) => (control.attributes('title') ?? '').length > 0);

    expect(titled).toHaveLength(FILTER_CHIPS.length);
    expect(toggle(wrapper, 'Unpushed').attributes('title')).toContain('last fetch');
    expect(toggle(wrapper, 'Stale fetch').attributes('title')).toContain('never');
  });

  /**
   * A chip is a two-state control, so it has to announce that state rather than only draw it.
   */
  it('announces which chips are pressed', () => {
    const wrapper = mountBar({ chips: ['dirty'] });

    expect(toggle(wrapper, 'Dirty').attributes('aria-pressed')).toBe('true');
    expect(toggle(wrapper, 'Detached').attributes('aria-pressed')).toBe('false');
  });

  it('emits the chip that was clicked', async () => {
    const wrapper = mountBar();

    await toggle(wrapper, 'Conflicted').trigger('click');

    expect(wrapper.emitted('chip')).toEqual([['conflicted']]);
  });

  it('emits the query as it is typed', async () => {
    const wrapper = mountBar();

    await wrapper.find('input').setValue('alpha');

    expect(wrapper.emitted('update:query')).toEqual([['alpha']]);
  });

  /**
   * **The honest half of a filter.** A chip that hides rows whose tier has not run must say so, or
   * an uncounted row reads as one that did not match — the uncomputed-renders-as-zero bug in filter
   * form.
   */
  it('says how many hidden rows are still being counted', () => {
    const wrapper = mountBar({
      chips: ['dirty'],
      counts: { visible: 3, hidden: 9, pending: 4 },
      total: 12,
    });

    expect(wrapper.text()).toContain('Showing 3 of 12');
    expect(wrapper.text()).toContain('4 still counting');
  });

  /**
   * And it must not claim work is in progress when none is: with every row judged, there is nothing
   * still counting.
   */
  it('says nothing about counting when every row has been judged', () => {
    const wrapper = mountBar({
      chips: ['dirty'],
      counts: { visible: 3, hidden: 9, pending: 0 },
      total: 12,
    });

    expect(wrapper.text()).toContain('Showing 3 of 12');
    expect(wrapper.text()).not.toContain('still counting');
  });

  /**
   * Nothing hidden means nothing to explain. A filter that matches every row should not caption a
   * table that looks exactly as it did before.
   */
  it('stays quiet when nothing is hidden', () => {
    const wrapper = mountBar({
      chips: ['dirty'],
      counts: { visible: 12, hidden: 0, pending: 0 },
      total: 12,
    });

    expect(wrapper.text()).not.toContain('Showing');
  });

  it('offers a way out of a filter that is hiding rows', async () => {
    const wrapper = mountBar({
      chips: ['dirty'],
      counts: { visible: 0, hidden: 12, pending: 0 },
      total: 12,
    });

    await toggle(wrapper, 'Clear filters').trigger('click');

    expect(wrapper.emitted('clear')).toHaveLength(1);
  });

  it('disables every control when the backend is unreachable', () => {
    const wrapper = mountBar({ disabled: true });

    expect(wrapper.find('input').attributes('disabled')).toBeDefined();
    expect(toggle(wrapper, 'Dirty').attributes('disabled')).toBeDefined();
  });
});
