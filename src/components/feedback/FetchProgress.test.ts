import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { FetchProgress as FetchProgressState } from '@/stores/repos';
import FetchProgress from './FetchProgress.vue';

function mountProgress(
  progress: Partial<FetchProgressState> = {},
  fetchErrors = new Map<string, string>(),
) {
  return mount(FetchProgress, {
    props: {
      progress: { total: 10, settled: 4, running: 2, failed: 0, ...progress },
      fetchErrors,
    },
  });
}

describe('FetchProgress', () => {
  /**
   * **The deliberate inverse of `ScanProgress`.** A scan's bar is indeterminate while discovery
   * runs because the denominator does not exist yet; a fetch is handed its exact list before the
   * first process starts, so an indeterminate bar here would claim ignorance of something known.
   */
  it('reports a known denominator as a determinate bar', () => {
    const wrapper = mountProgress();

    const bar = wrapper.find('[role="progressbar"]');
    expect(bar.attributes('aria-valuenow')).toBe('4');
    expect(bar.attributes('aria-valuemax')).toBe('10');
  });

  /**
   * The numerator is settled repositories, never started ones — otherwise the bar reads 100% with
   * four fetches still running.
   */
  it('counts a repository once it has settled, not once it has started', () => {
    const wrapper = mountProgress({ total: 10, settled: 6, running: 4 });

    expect(wrapper.text()).toContain('Fetched 6 of 10');
    expect(wrapper.text()).toContain('4 running');
  });

  /**
   * The bar cannot carry the honest part: 299 local repositories finish in a second and one VPN'd
   * monorepo takes a minute, so it sits at 99% for most of the wall clock.
   */
  it('says how many failed rather than only how many are done', () => {
    const wrapper = mountProgress({ total: 10, settled: 10, running: 0, failed: 3 });

    expect(wrapper.text()).toContain('3 failed');
  });

  it('says nothing about running or failed when there is neither', () => {
    const wrapper = mountProgress({ total: 4, settled: 4, running: 0, failed: 0 });

    expect(wrapper.text()).toContain('Fetched 4 of 4');
    expect(wrapper.text()).not.toContain('running');
    expect(wrapper.text()).not.toContain('failed');
  });

  /**
   * Naming them, not just counting them — and through the panel the scan already uses, because a
   * fetch failure has the same shape as a read failure.
   */
  it('names the failures rather than only counting them', () => {
    const wrapper = mountProgress(
      { failed: 1 },
      new Map([['C:/work/alpha', 'fatal: Authentication failed']]),
    );

    expect(wrapper.text()).toContain('C:/work/alpha');
    expect(wrapper.text()).toContain('fatal: Authentication failed');
  });
});
