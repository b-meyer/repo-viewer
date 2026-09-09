import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { RepoRow } from '@/stores/repos';
import { MakeDiscovered, MakeStatus } from '@/tests/fixtures';
import RepoTable from './RepoTable.vue';

const NOW = 1_700_000_000_000;

function mountTable(
  rows: RepoRow[],
  extra: {
    repoErrors?: Map<string, string>;
    tier0Done?: boolean;
    expanded?: Set<string>;
    loadingDetail?: Set<string>;
    detailErrors?: Map<string, string>;
  } = {},
) {
  return mount(RepoTable, {
    props: {
      rows,
      now: NOW,
      repoErrors: extra.repoErrors ?? new Map(),
      tier0Done: extra.tier0Done ?? false,
      expanded: extra.expanded ?? new Set<string>(),
      loadingDetail: extra.loadingDetail ?? new Set<string>(),
      detailErrors: extra.detailErrors ?? new Map(),
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
});
