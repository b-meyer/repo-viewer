import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { ScanProgress as Progress } from '@/stores/repos';
import { MakeTotals } from '@/tests/fixtures';
import ScanProgress from './ScanProgress.vue';

function mountProgress(progress: Partial<Progress> = {}, summaries = {}) {
  return mount(ScanProgress, {
    props: {
      progress: { phase: 'discovering', found: 0, read: 0, total: null, ...progress },
      discovery: null,
      totals: null,
      repoErrors: new Map<string, string>(),
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

  /**
   * The scan total and the per-tier figures are different measurements and all four are shown.
   * `scan` is wall clock and is deliberately **not** the sum of the rest: most of a scan is spent
   * waiting for the walk to hand over the next batch, which belongs to no tier.
   */
  it('reports the scan total and each tier separately', () => {
    const wrapper = mountProgress(
      { phase: 'done', found: 3, read: 3, total: 3 },
      {
        discovery: { reposFound: 3, dirsVisited: 90, dirsPruned: 2, errors: [], elapsedMs: 58 },
        totals: MakeTotals({
          reposRead: 3,
          elapsedMs: 8000,
          discoveryMs: 58,
          tier0Ms: 322,
          tier1Ms: 7581,
        }),
      },
    );

    expect(wrapper.text()).toContain('3 repositories');
    expect(wrapper.text()).toContain('scan 8000 ms');
    expect(wrapper.text()).toContain('discovery 58 ms');
    // The gap between these two is the entire argument for streaming the tiers apart, so the UI
    // says both rather than one number standing for the pair.
    expect(wrapper.text()).toContain('tier 0 322 ms');
    expect(wrapper.text()).toContain('tier 1 7581 ms');
  });

  /**
   * Built from the accumulating map, not from the terminal summary, so a failure shows up while the
   * scan is still running.
   */
  it('lists the paths a scan could not read, before it finishes', () => {
    const wrapper = mountProgress(
      { phase: 'reading', read: 1, total: 2 },
      { repoErrors: new Map([['C:/bad', 'HEAD unreadable']]) },
    );

    expect(wrapper.text()).toContain('1 path could not be read');
    expect(wrapper.text()).toContain('HEAD unreadable');
  });
});
