import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import type { RepoState } from '@/scripts/generated/RepoState';
import RepoStateBadge from './RepoStateBadge.vue';

describe('RepoStateBadge', () => {
  /**
   * A badge on every row would drown the five that matter.
   */
  it('renders nothing at all for a clean repository', () => {
    expect(mount(RepoStateBadge, { props: { state: 'clean' } }).text()).toBe('');
  });

  it('names every parked operation', () => {
    const states: RepoState[] = ['merging', 'rebasing', 'bisecting', 'cherryPicking', 'reverting'];

    for (const state of states) {
      expect(mount(RepoStateBadge, { props: { state } }).text()).not.toBe('');
    }
  });
});
