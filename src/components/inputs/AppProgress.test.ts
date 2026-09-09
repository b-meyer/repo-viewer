import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import AppProgress from './AppProgress.vue';

describe('AppProgress', () => {
  /**
   * An unknown total must render as indeterminate, never as a zero-width determinate bar — which
   * would read as "nothing done" when the truth is "we do not know how much there is".
   */
  it('renders indeterminate when the value is unknown', () => {
    const wrapper = mount(AppProgress, { props: { value: null, max: 1, label: 'Repositories' } });

    expect(wrapper.html()).toContain('data-state="indeterminate"');
  });

  it('renders determinate once the total is known', () => {
    const wrapper = mount(AppProgress, { props: { value: 4, max: 8, label: 'Repositories' } });

    expect(wrapper.html()).not.toContain('data-state="indeterminate"');
    expect(wrapper.html()).toContain('50%');
  });
});
