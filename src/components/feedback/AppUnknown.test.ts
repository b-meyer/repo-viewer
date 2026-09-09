import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import AppUnknown from './AppUnknown.vue';

describe('AppUnknown', () => {
  /**
   * The four reasons must stay four distinct strings. Collapsing any two of them loses the
   * distinction between "not counted yet", "there is none", "cannot apply" and "we failed" — which
   * is the whole reason this component is the single definition of an absent value.
   */
  it('renders four distinct labels, none of them zero or empty', () => {
    const reasons = ['pending', 'none', 'na', 'unreadable'] as const;
    const rendered = reasons.map((reason) => mount(AppUnknown, { props: { reason } }).text());

    expect(new Set(rendered).size).toBe(reasons.length);
    for (const text of rendered) {
      expect(text).not.toBe('');
      expect(text).not.toBe('0');
    }
  });

  it('uses the wording the design calls for when a tier has not run', () => {
    expect(mount(AppUnknown, { props: { reason: 'pending' } }).text()).toBe('counting…');
  });

  it('carries the cause as hover text', () => {
    const wrapper = mount(AppUnknown, {
      props: { reason: 'unreadable', hint: 'HEAD is corrupt' },
    });

    expect(wrapper.attributes('title')).toBe('HEAD is corrupt');
  });
});
