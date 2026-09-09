import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import RepoHead from './RepoHead.vue';

describe('RepoHead', () => {
  it('names a branch', () => {
    expect(mount(RepoHead, { props: { head: { kind: 'branch', name: 'main' } } }).text()).toBe(
      'main',
    );
  });

  it('abbreviates a detached commit and keeps the full id in hover text', () => {
    const id = '0123456789abcdef0123456789abcdef01234567';
    const wrapper = mount(RepoHead, { props: { head: { kind: 'detached', id } } });

    expect(wrapper.text()).toBe('0123456');
    expect(wrapper.html()).toContain(id);
  });

  /**
   * An unborn HEAD is a real state, not a missing value: the branch exists with no commits.
   */
  it('names an unborn head rather than showing nothing', () => {
    const wrapper = mount(RepoHead, { props: { head: { kind: 'unborn' } } });

    expect(wrapper.text()).toBe('unborn');
  });
});
