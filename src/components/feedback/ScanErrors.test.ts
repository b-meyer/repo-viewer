import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vite-plus/test';
import ScanErrors from './ScanErrors.vue';

describe('ScanErrors', () => {
  it('renders nothing when there were no failures', () => {
    expect(mount(ScanErrors, { props: { errors: [], title: 'could not be read' } }).text()).toBe(
      '',
    );
  });

  it('counts the failures and lists their causes', () => {
    const wrapper = mount(ScanErrors, {
      props: {
        errors: [
          { path: 'C:/a', message: 'permission denied' },
          { path: 'C:/b', message: 'HEAD unreadable' },
        ],
        title: 'could not be read',
      },
    });

    expect(wrapper.text()).toContain('2 paths could not be read');
    expect(wrapper.text()).toContain('permission denied');
  });

  it('uses the singular for one failure', () => {
    const wrapper = mount(ScanErrors, {
      props: { errors: [{ path: 'C:/a', message: 'x' }], title: 'could not be read' },
    });

    expect(wrapper.text()).toContain('1 path could not be read');
  });
});
