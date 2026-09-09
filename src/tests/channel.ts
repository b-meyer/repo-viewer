/**
 * Drives a mocked `Channel` the way Tauri's runtime does.
 *
 * Under `mockIPC` the npm package's `invoke` is a bare pass-through and **no serialization runs**,
 * so a handler receives the _live_ `Channel` instance as `args.onEvent` — not the
 * `"__CHANNEL__:<id>"` string the real IPC bridge would produce. That is what makes streaming
 * testable with `mockIPC` alone, with no hand-rolled stub and no extra hook.
 */

/**
 * The subset of Tauri's mock internals these helpers reach for.
 */
type TauriInternals = {
  /**
   * Delivers a raw message to the callback registered under `id`.
   */
  runCallback: (id: number, payload: unknown) => void;
};

declare global {
  /* eslint-disable no-var, vars-on-top -- `declare global` needs `var` to declare a binding on
     `globalThis`; this is the only shape TypeScript accepts here. */
  var __TAURI_INTERNALS__: TauriInternals;
  /* eslint-enable no-var, vars-on-top */
}

/**
 * Sends messages to channels, keeping each one's message index.
 */
export type ChannelDriver = {
  /**
   * Pulls the live `Channel`'s id out of a mocked invoke's arguments.
   */
  Capture: (args: unknown) => number;
  /**
   * Delivers one message, in order.
   */
  Send: (id: number, message: unknown) => void;
  /**
   * Closes the channel, as Rust's drop hook does.
   */
  End: (id: number) => void;
};

/**
 * Reads a number from a nested field of an unknown value.
 *
 * Narrowed rather than asserted so that a changed wire shape fails with a sentence naming the
 * field, instead of surfacing later as `undefined` somewhere unrelated.
 *
 * @param source - The value to read from.
 * @param path - Field names to walk, outermost first.
 * @returns The number found there.
 */
export function ReadNumber(source: unknown, ...path: string[]): number {
  let current: unknown = source;
  for (const key of path) {
    if (typeof current !== 'object' || current === null || !(key in current)) {
      throw new Error(`expected an object with \`${key}\`, got ${JSON.stringify(source)}`);
    }
    current = Reflect.get(current, key);
  }
  if (typeof current !== 'number') {
    throw new Error(`expected \`${path.join('.')}\` to be a number, got ${typeof current}`);
  }
  return current;
}

/**
 * Builds a driver for one test.
 *
 * `clearMocks()` invalidates every channel from a previous test, so a driver must not be shared
 * across them.
 *
 * @returns Handles for capturing and feeding channels.
 */
export function MakeChannelDriver(): ChannelDriver {
  const counters = new Map<number, number>();

  return {
    Capture(args: unknown): number {
      return ReadNumber(args, 'onEvent', 'id');
    },

    /**
     * `index` must start at 0 and increase by exactly 1. `Channel` buffers an out-of-order message
     * in a private field and delivers nothing until the gap fills — which presents as a test
     * hanging on an assertion rather than as an error, so the counter is kept here rather than left
     * to each test.
     */
    Send(id: number, message: unknown): void {
      const index = counters.get(id) ?? 0;
      counters.set(id, index + 1);
      globalThis.__TAURI_INTERNALS__.runCallback(id, { index, message });
    },

    End(id: number): void {
      globalThis.__TAURI_INTERNALS__.runCallback(id, { index: counters.get(id) ?? 0, end: true });
    },
  };
}
