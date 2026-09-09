import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { RepoRow } from '@/stores/repos';
import { MakeDiscovered, MakeStatus } from '@/tests/fixtures';
import RepoTable from './RepoTable.vue';

const NOW = 1_700_000_000_000;

function mountTable(rows: RepoRow[], readErrors: Map<string, string> | null = null) {
  return mount(RepoTable, {
    props: { rows, now: NOW, readErrors, emptyMessage: 'Nothing here.' },
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
   * A `null` map means Tier 0 is still running, which is what a row needs to know.
   */
  it('tells rows Tier 0 has finished only once it has', () => {
    expect(mountTable([MakeDiscovered()]).text()).toContain('counting…');
    expect(mountTable([MakeDiscovered()], new Map()).text()).toContain('unreadable');
  });
});
