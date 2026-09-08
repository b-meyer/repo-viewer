import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { flushPromises, mount } from '@vue/test-utils';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import IndexPage from './index.vue';

describe('index page', () => {
  afterEach(() => {
    clearMocks();
  });

  it('renders the backend reply once the ping resolves', async () => {
    mockIPC(() => 'pong');

    const wrapper = mount(IndexPage);
    await flushPromises();

    expect(wrapper.text()).toContain('Connected');
    expect(wrapper.find('code').text()).toBe('pong');
  });

  it('shows the failure state rather than an empty card when IPC is unavailable', async () => {
    mockIPC(() => {
      throw new Error('no backend');
    });

    const wrapper = mount(IndexPage);
    await flushPromises();

    // The point of the three-state render: a broken bridge must not look like a blank page.
    expect(wrapper.text()).toContain('IPC unavailable');
    expect(wrapper.text()).toContain('no backend');
  });
});
