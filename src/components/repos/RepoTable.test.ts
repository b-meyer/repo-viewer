import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { SortDirection, SortKey } from '@/scripts/settings';
import type { RepoGroup } from '@/scripts/view';
import type { RepoRow } from '@/stores/repos';
import { MakeDiscovered, MakeStatus } from '@/tests/fixtures';
import RepoTable from './RepoTable.vue';

const NOW = 1_700_000_000_000;

function mountTable(
  rows: RepoRow[],
  extra: {
    groups?: RepoGroup[];
    sortKey?: SortKey;
    sortDirection?: SortDirection;
    repoErrors?: Map<string, string>;
    tier0Done?: boolean;
    expanded?: Set<string>;
    loadingDetail?: Set<string>;
    detailErrors?: Map<string, string>;
    openErrors?: Map<string, string>;
  } = {},
) {
  return mount(RepoTable, {
    props: {
      groups: extra.groups ?? [{ key: '', rows }],
      sortKey: extra.sortKey ?? 'name',
      sortDirection: extra.sortDirection ?? 'asc',
      now: NOW,
      repoErrors: extra.repoErrors ?? new Map(),
      tier0Done: extra.tier0Done ?? false,
      expanded: extra.expanded ?? new Set<string>(),
      loadingDetail: extra.loadingDetail ?? new Set<string>(),
      detailErrors: extra.detailErrors ?? new Map(),
      openErrors: extra.openErrors ?? new Map(),
      fetchStates: new Map(),
      fetchErrors: new Map(),
      gitMissing: null,
      emptyMessage: 'Nothing here.',
    },
  });
}

describe('RepoTable', () => {
  it('shows the empty message rather than an empty table', () => {
    const wrapper = mountTable([]);

    expect(wrapper.text()).toContain('Nothing here.');
    expect(wrapper.find('table').exists()).toBe(false);
  });

  it('renders one row per repository', () => {
    const wrapper = mountTable([
      MakeStatus({ path: 'C:/a', name: 'a' }),
      MakeDiscovered({ path: 'C:/b', name: 'b' }),
    ]);

    expect(wrapper.findAll('tbody tr')).toHaveLength(2);
  });

  /**
   * The counts and the fetch age share a column so that no layout change can separate them: they
   * are only meaningful together.
   */
  it('keeps upstream and sync in a single column', () => {
    const headers = mountTable([MakeStatus()])
      .findAll('th')
      .map((th) => th.text());

    expect(headers).toContain('Upstream & sync');
    expect(headers).not.toContain('Last fetched');
  });

  /**
   * `tier0Done` is what turns a row's "not yet" into "never", and it is a fact of its own rather
   * than something inferred from the error map being absent.
   */
  it('tells rows Tier 0 has finished only once it has', () => {
    expect(mountTable([MakeDiscovered()]).text()).toContain('counting…');
    expect(mountTable([MakeDiscovered()], { tier0Done: true }).text()).toContain('unreadable');
  });

  /**
   * The other half of that split: a failure Rust reported mid-scan reaches the row immediately,
   * without waiting for the scan to finish.
   */
  it('passes a mid-scan read failure straight to its row', () => {
    const wrapper = mountTable([MakeDiscovered({ path: 'C:/a' })], {
      repoErrors: new Map([['C:/a', 'HEAD is corrupt']]),
    });

    expect(wrapper.text()).toContain('unreadable');
    expect(wrapper.text()).not.toContain('counting…');
    expect(wrapper.html()).toContain('HEAD is corrupt');
  });

  /**
   * The drawer is a second `<tr>`, because a `<td>` cannot hold something that spans the table.
   */
  it('renders an expanded row as a second row spanning every column', () => {
    const columns = mountTable([MakeStatus({ path: 'C:/a' })]).findAll('th').length;
    const wrapper = mountTable([MakeStatus({ path: 'C:/a' })], {
      expanded: new Set(['C:/a']),
    });

    const rows = wrapper.findAll('tbody tr');
    expect(rows).toHaveLength(2);
    expect(rows[1]?.find('td').attributes('colspan')).toBe(String(columns));
  });

  it('emits the path when a row asks to be toggled', async () => {
    const wrapper = mountTable([MakeStatus({ path: 'C:/a' })]);

    await wrapper.find('tbody button').trigger('click');

    expect(wrapper.emitted('toggle')).toEqual([['C:/a']]);
  });

  /**
   * Every header is a `SortKey`, so a click resolves to exactly one comparator rather than to a
   * mapping kept somewhere between the table and `view.ts`.
   */
  it('emits a sort key when a header is clicked', async () => {
    const wrapper = mountTable([MakeStatus()]);

    await wrapper.findAll('thead button')[1]?.trigger('click');

    expect(wrapper.emitted('sort')).toEqual([['head']]);
  });

  /**
   * The sort has to be announced, not just drawn: an arrow is invisible to a screen reader, and
   * `aria-sort` belongs on the cell rather than on the button inside it.
   */
  it('announces which column is sorted, and which way', () => {
    const wrapper = mountTable([MakeStatus()], { sortKey: 'read', sortDirection: 'desc' });
    const sorted = wrapper.findAll('th').map((th) => th.attributes('aria-sort'));

    expect(sorted).toContain('descending');
    expect(sorted.filter((value) => value !== 'none')).toHaveLength(1);
    expect(wrapper.findAll('th').at(-1)?.attributes('aria-sort')).toBe('descending');
  });

  /**
   * Grouping adds a header row per folder and leaves the rows themselves alone — one `tbody` per
   * group, which is what lets the header be a real `<tr>` inside the table.
   */
  it('renders a header row per group when grouping is on', () => {
    const wrapper = mountTable([], {
      groups: [
        { key: 'C:/work', rows: [MakeStatus({ path: 'C:/work/a', name: 'a' })] },
        { key: 'C:/other', rows: [MakeStatus({ path: 'C:/other/b', name: 'b' })] },
      ],
    });

    expect(wrapper.findAll('tbody')).toHaveLength(2);
    expect(wrapper.text()).toContain('C:/work');
    expect(wrapper.text()).toContain('C:/other');
    expect(wrapper.findAll('th[scope="colgroup"]')).toHaveLength(2);
  });

  /**
   * With grouping off there is one group with no key, and it must not draw a header for it.
   */
  it('draws no group header when there is no group', () => {
    const wrapper = mountTable([MakeStatus()]);

    expect(wrapper.find('th[scope="colgroup"]').exists()).toBe(false);
  });
});
