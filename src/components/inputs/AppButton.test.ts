import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import AppButton from './AppButton.vue';

describe('AppButton', () => {
  /**
   * `main.css` nests the companions inside `@utility btn`, so dropping `btn` loses every rule.
   */
  it('keeps the base class alongside its companion', () => {
    const wrapper = mount(AppButton, {
      props: { label: 'Scan', variant: 'primary', size: 'small' },
    });

    expect(wrapper.classes()).toContain('btn');
    expect(wrapper.classes()).toContain('btn-primary');
    expect(wrapper.classes()).toContain('btn-small');
  });

  it('emits a click when it is enabled', async () => {
    const wrapper = mount(AppButton, { props: { label: 'Scan' } });

    await wrapper.trigger('click');

    expect(wrapper.emitted('click')).toHaveLength(1);
  });

  it('disables itself while busy, so one action cannot be started twice', () => {
    const wrapper = mount(AppButton, { props: { label: 'Scan', busy: true } });

    expect(wrapper.attributes('disabled')).toBeDefined();
    expect(wrapper.html()).toContain('animate-spin');
  });
});
