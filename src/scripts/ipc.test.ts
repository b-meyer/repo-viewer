import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { ping } from '@/scripts/ipc';

/**
 * `mockIPC` intercepts `invoke` and `Channel` traffic without a webview, so the IPC layer is
 * testable without booting Tauri. It is the supported mechanism — no hand-rolled `invoke` stubs.
 */
describe('ipc', () => {
  afterEach(() => {
    clearMocks();
  });

  it('invokes the ping command and returns its reply', async () => {
    const calls: string[] = [];
    mockIPC((cmd) => {
      calls.push(cmd);
      return 'pong';
    });

    await expect(ping()).resolves.toBe('pong');
    expect(calls).toEqual(['ping']);
  });

  it('rejects when the command fails, rather than resolving to undefined', async () => {
    mockIPC(() => {
      throw new Error('command not found');
    });

    // The page renders a distinct state for this, so the rejection has to survive the call.
    await expect(ping()).rejects.toThrow('command not found');
  });
});
