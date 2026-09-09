import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import AppToggle from './AppToggle.vue';

function mountToggle(
  pressed: boolean,
  extra: { count?: number | null; title?: string; disabled?: boolean } = {},
) {
  return mount(AppToggle, {
    props: {
      modelValue: pressed,
      label: 'Dirty',
      icon: 'bi-pencil-fill',
      count: extra.count ?? null,
      title: extra.title ?? '',
      disabled: extra.disabled ?? false,
    },
  });
}

describe('AppToggle', () => {
  /**
   * The reason this wraps a primitive rather than styling a button: a two-state control has to
   * announce its state, and `aria-pressed` is how. A coloured button announces nothing.
   */
  it('renders a button that announces its pressed state', () => {
    expect(mountToggle(false).find('button').attributes('aria-pressed')).toBe('false');
    expect(mountToggle(true).find('button').attributes('aria-pressed')).toBe('true');
  });

  it('emits the new state when clicked', async () => {
    const wrapper = mountToggle(false);

    await wrapper.find('button').trigger('click');

    expect(wrapper.emitted('update:modelValue')).toEqual([[true]]);
  });

  /**
   * `as-child` is what keeps the rendered element our own button, so an attribute a plain button
   * accepts — a tooltip — still reaches it.
   */
  it('carries a tooltip', () => {
    const wrapper = mountToggle(false, { title: 'Uncommitted changes.' });

    expect(wrapper.find('button').attributes('title')).toBe('Uncommitted changes.');
  });

  /**
   * `null` and `0` are different things to say, so the count is only drawn when there is one.
   */
  it('shows a count only when it has one', () => {
    expect(mountToggle(true, { count: 0 }).text()).toContain('0');
    expect(mountToggle(true, { count: null }).text()).toBe('Dirty');
  });

  it('does not emit while disabled', async () => {
    const wrapper = mountToggle(false, { disabled: true });

    await wrapper.find('button').trigger('click');

    expect(wrapper.emitted('update:modelValue')).toBeUndefined();
  });
});
