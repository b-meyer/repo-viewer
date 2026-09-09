import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { ScanProgress as Progress } from '@/stores/repos';
import ScanProgress from './ScanProgress.vue';

function mountProgress(progress: Partial<Progress> = {}, summaries = {}) {
  return mount(ScanProgress, {
    props: {
      progress: { phase: 'discovering', found: 0, read: 0, total: null, ...progress },
      discovery: null,
      tier0: null,
      ...summaries,
    },
  });
}

describe('ScanProgress', () => {
  /**
   * While the walk is running there is no honest denominator, so none is shown. A total that keeps
   * revising itself downward reads as a bug even when it is not.
   */
  it('shows a count with no denominator while discovering', () => {
    const wrapper = mountProgress({ phase: 'discovering', found: 128 });

    expect(wrapper.text()).toContain('128 found');
    expect(wrapper.text()).not.toContain(' of ');
    expect(wrapper.html()).toContain('data-state="indeterminate"');
  });

  it('shows a denominator once the walk has finished', () => {
    const wrapper = mountProgress({ phase: 'reading', found: 128, read: 40, total: 128 });

    expect(wrapper.text()).toContain('Read 40 of 128');
  });

  it('reports what a cancelled scan managed', () => {
    const wrapper = mountProgress({ phase: 'cancelled', found: 128, read: 40 });

    expect(wrapper.text()).toContain('Cancelled after 40 of 128');
  });

  it('summarises timings when the scan is done', () => {
    const wrapper = mountProgress(
      { phase: 'done', found: 3, read: 3, total: 3 },
      {
        discovery: { reposFound: 3, dirsVisited: 90, dirsPruned: 2, errors: [], elapsedMs: 58 },
        tier0: { reposRead: 3, errors: [], elapsedMs: 146 },
      },
    );

    expect(wrapper.text()).toContain('3 repositories');
    expect(wrapper.text()).toContain('discovery 58 ms');
    // `scan`, not `Tier 0`. The pipeline puts its whole wall-clock time in this field — walk and
    // Tier 1 included — so naming the cheap tier there blames it for the expensive one's cost.
    expect(wrapper.text()).toContain('scan 146 ms');
    expect(wrapper.text()).not.toContain('Tier 0');
  });

  it('lists the paths a scan could not read', () => {
    const wrapper = mountProgress(
      { phase: 'done', read: 1, total: 2 },
      {
        tier0: {
          reposRead: 1,
          errors: [{ path: 'C:/bad', message: 'HEAD unreadable' }],
          elapsedMs: 5,
        },
      },
    );

    expect(wrapper.text()).toContain('1 path could not be read');
    expect(wrapper.text()).toContain('HEAD unreadable');
  });
});
