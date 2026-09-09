import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { ResetScanSession } from '@/scripts/scan';
import App from './App.vue';

/**
 * Mounts the shell with the router outlet stubbed — routing is not what these tests are about.
 */
function mountApp() {
  return mount(App, { global: { stubs: { RouterView: true } } });
}

describe('App shell', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  afterEach(() => {
    clearMocks();
    ResetScanSession();
  });

  it('boots quietly when the bridge is up', async () => {
    mockIPC((cmd) => (cmd === 'ping' ? 'pong' : []));

    const wrapper = mountApp();
    await flushPromises();

    expect(wrapper.text()).not.toContain('The backend is unavailable');
  });

  /**
   * The reason `ping` survives. A broken bridge must not present as a blank page, and it is a
   * different diagnosis from `subscribe` alone failing.
   */
  it('shows a failure banner rather than an empty page when IPC is down', async () => {
    mockIPC(() => {
      throw new Error('no backend');
    });

    const wrapper = mountApp();
    await flushPromises();

    expect(wrapper.text()).toContain('The backend is unavailable');
    expect(wrapper.text()).toContain('no backend');
  });

  it('mirrors the rows the session snapshot returns', async () => {
    mockIPC((cmd) => {
      if (cmd === 'ping') return 'pong';
      if (cmd === 'subscribe') return [];
      return null;
    });

    const wrapper = mountApp();
    await flushPromises();

    expect(wrapper.text()).not.toContain('The backend is unavailable');
  });
});
