import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import AppAlert from './AppAlert.vue';

describe('AppAlert', () => {
  it('renders its title and body', () => {
    const wrapper = mount(AppAlert, {
      props: { tone: 'error', title: 'The scan failed' },
      slots: { default: 'not a configured root' },
    });

    expect(wrapper.text()).toContain('The scan failed');
    expect(wrapper.text()).toContain('not a configured root');
  });

  it('colours itself by tone', () => {
    const error = mount(AppAlert, { props: { tone: 'error', title: 'x' } });
    const info = mount(AppAlert, { props: { tone: 'info', title: 'x' } });

    expect(error.html()).toContain('red');
    expect(info.html()).toContain('blue');
  });
});
