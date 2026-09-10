import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { createPinia, setActivePinia } from 'pinia';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { EnsureDetail, OpenIn, RefreshDetail, ToggleRow } from '@/scripts/detail';
import { useReposStore } from '@/stores/repos';
import { MakeCounts, MakeDiscovered, MakeStatus } from '@/tests/fixtures';

/**
 * Records every command invoked, and answers `full_status` / `refresh_repo` with a row carrying
 * counts — which is what Rust does, since both resolve with the whole merged row.
 */
function mockReads(overrides: Parameters<typeof MakeStatus>[0] = {}) {
  const calls: string[] = [];
  mockIPC((cmd) => {
    calls.push(cmd);
    if (cmd === 'full_status' || cmd === 'refresh_repo') {
      return MakeStatus({
        path: 'C:/work/alpha',
        counts: MakeCounts(),
        submodules: [],
        ...overrides,
      });
    }
    return null;
  });
  return calls;
}

describe('row detail', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  afterEach(() => {
    clearMocks();
  });

  it('reads Tier 2 the first time a row is expanded', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha' }));
    const calls = mockReads();

    await ToggleRow('C:/work/alpha');

    expect(calls).toEqual(['full_status']);
    expect(store.expanded.has('C:/work/alpha')).toBe(true);
    expect(store.byPath.get('C:/work/alpha')).toMatchObject({ counts: MakeCounts() });
  });

  /**
   * **The phase's deliverable.** Rust keeps what it read, and the store is a mirror of Rust, so a
   * row that has been expanded once still has its counts after the drawer closes. Re-expanding
   * therefore paints immediately and asks for nothing.
   *
   * A cache kept in this module would pass this test too — and would then have to be invalidated,
   * which is why there is not one. The counts come back from the mirror.
   */
  it('keeps its counts across a collapse and does not read them twice', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha' }));
    const calls = mockReads();

    await ToggleRow('C:/work/alpha'); // expand — reads
    await ToggleRow('C:/work/alpha'); // collapse
    expect(store.expanded.has('C:/work/alpha')).toBe(false);
    expect(store.byPath.get('C:/work/alpha')).toMatchObject({
      counts: MakeCounts(),
    });

    await ToggleRow('C:/work/alpha'); // expand again — must not read

    expect(calls).toEqual(['full_status']);
    expect(store.expanded.has('C:/work/alpha')).toBe(true);
    expect(store.byPath.get('C:/work/alpha')).toMatchObject({ counts: MakeCounts() });
  });

  /**
   * An unexpanded row has no counts, and the absence must stay an absence: this is the rule the
   * whole design turns on, checked at the layer that decides when to read.
   */
  it('leaves an unexpanded row with unknown counts rather than zeroes', () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha' }));

    const row = store.byPath.get('C:/work/alpha');

    expect(row).toMatchObject({ counts: null, submodules: null });
  });

  /**
   * Collapsing must not fire a read, and neither must the toggle that closes a row mid-flight.
   */
  it('reads nothing when a row is collapsed', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha', counts: MakeCounts() }));
    store.ToggleExpanded('C:/work/alpha');
    const calls = mockReads();

    await ToggleRow('C:/work/alpha');

    expect(calls).toEqual([]);
    expect(store.expanded.has('C:/work/alpha')).toBe(false);
  });

  /**
   * A row Tier 0 never read has no `RepoStatus` to fill in, and Rust refuses the command for that
   * reason. Opening the drawer is still allowed — it is where the failure gets explained — but no
   * command goes out.
   */
  it('opens a discovered-only row without asking Rust for counts', async () => {
    const store = useReposStore();
    store.Upsert(MakeDiscovered({ path: 'C:/work/broken' }));
    const calls = mockReads();

    await ToggleRow('C:/work/broken');

    expect(calls).toEqual([]);
    expect(store.expanded.has('C:/work/broken')).toBe(true);
  });

  /**
   * A bare repository has no worktree and no `.gitmodules`, so both halves are `n/a` forever. Rust
   * answers correctly if asked, but the answer can never populate `counts` — so without this guard
   * the "already read" check never fires and **every** expand spends a round trip to be told the
   * same nothing.
   */
  it('never asks Rust for counts a bare repository cannot have', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/mirror.git', kind: 'bare' }));
    const calls = mockReads();

    await ToggleRow('C:/work/mirror.git');
    await ToggleRow('C:/work/mirror.git');
    await ToggleRow('C:/work/mirror.git');

    expect(calls).toEqual([]);
    expect(store.expanded.has('C:/work/mirror.git')).toBe(true);
  });

  it('records a failed read against the row and clears the in-flight flag', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha' }));
    mockIPC(() => {
      throw new Error('index unreadable');
    });

    await ToggleRow('C:/work/alpha');

    expect(store.detailErrors.get('C:/work/alpha')).toContain('index unreadable');
    // `counting…` is a claim that work is happening, so it has to stop when the work does — on the
    // failure path just as much as on the successful one.
    expect(store.loadingDetail.has('C:/work/alpha')).toBe(false);
    expect(store.byPath.get('C:/work/alpha')).toMatchObject({ counts: null });
  });

  /**
   * The obligation a watcher refresh creates. It invalidates Tier 2 for a repository that changed,
   * so `counts` goes back to `null` on a row whose drawer is already open — and `counting…` is a
   * claim about work in progress, so something has to make the claim true.
   */
  it('re-reads counts that were invalidated under an open drawer', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha', counts: MakeCounts() }));
    await ToggleRow('C:/work/alpha');

    // What the watcher's push looks like on this side: the same row, Tier 2 cleared.
    store.Upsert(MakeStatus({ path: 'C:/work/alpha', counts: null }));
    const calls = mockReads({ counts: MakeCounts({ staged: 5 }) });
    await EnsureDetail('C:/work/alpha');

    expect(calls).toEqual(['full_status']);
    expect(store.byPath.get('C:/work/alpha')).toMatchObject({
      counts: MakeCounts({ staged: 5 }),
    });
  });

  /**
   * And it is free when there is nothing to do, which is what makes it safe to call for every
   * expanded row in every update the session channel delivers.
   */
  it('asks for nothing when the counts are already there', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha', counts: MakeCounts() }));
    const calls = mockReads();

    await EnsureDetail('C:/work/alpha');

    expect(calls).toEqual([]);
  });

  /**
   * A read already in flight is the common case for a watcher push: the row lands, this runs, and
   * the very next batch must not start a second read of the same repository.
   */
  it('does not start a second read while one is in flight', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha', counts: null }));
    store.SetLoadingDetail('C:/work/alpha', true);
    const calls = mockReads();

    await EnsureDetail('C:/work/alpha');

    expect(calls).toEqual([]);
  });

  /**
   * A re-read is explicit, and cumulative: `'two'` re-reads the refs and the worktree as well,
   * because counts describe a worktree relative to a branch and the pair must be one moment.
   */
  it('re-reads on request at the cumulative top tier', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha', counts: MakeCounts() }));
    const tiers: unknown[] = [];
    mockIPC((cmd, args) => {
      if (cmd !== 'refresh_repo') return null;
      // Narrowed rather than asserted, so a changed argument shape fails here instead of pushing
      // `undefined` and passing.
      if (typeof args === 'object' && args !== null && 'tier' in args) {
        tiers.push(Reflect.get(args, 'tier'));
      }
      return MakeStatus({ path: 'C:/work/alpha', counts: MakeCounts({ staged: 9 }) });
    });

    await RefreshDetail('C:/work/alpha');

    expect(tiers).toEqual(['two']);
    expect(store.byPath.get('C:/work/alpha')).toMatchObject({
      counts: MakeCounts({ staged: 9 }),
    });
  });

  /**
   * A failed re-read leaves the previous values alone: they were really measured, and replacing
   * them with unknowns would discard a true answer in favour of no answer.
   */
  it('keeps the previous counts when a re-read fails', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha', counts: MakeCounts() }));
    mockIPC(() => {
      throw new Error('index unreadable');
    });

    await RefreshDetail('C:/work/alpha');

    expect(store.byPath.get('C:/work/alpha')).toMatchObject({ counts: MakeCounts() });
    expect(store.detailErrors.get('C:/work/alpha')).toContain('index unreadable');
  });

  /**
   * Clearing the previous failure before a retry is what stops a drawer showing a stale cause
   * beside numbers that have since been read successfully.
   */
  it('clears a previous failure when the read succeeds', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha' }));
    store.SetDetailError('C:/work/alpha', 'index unreadable');
    mockReads();

    await RefreshDetail('C:/work/alpha');

    expect(store.detailErrors.has('C:/work/alpha')).toBe(false);
  });
});

describe('OpenIn', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  afterEach(() => {
    clearMocks();
  });

  it('asks Rust to open the row, and says which target', async () => {
    const calls: unknown[] = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      return null;
    });

    await OpenIn('C:/work/alpha', 'terminal');

    expect(calls).toEqual([
      { cmd: 'open_in', args: { path: 'C:/work/alpha', target: 'terminal' } },
    ]);
  });

  /**
   * A launch failure belongs to the row and the button that produced it. It must not reach the
   * row's one `error` slot, which the tiers own, and it must not be reported as a failed read.
   */
  it('records a launch failure without touching the read state', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha' }));
    mockIPC(() => {
      throw new Error('could not run `code`');
    });

    await OpenIn('C:/work/alpha', 'editor');

    expect(store.openErrors.get('C:/work/alpha')).toContain('could not run `code`');
    expect(store.detailErrors.has('C:/work/alpha')).toBe(false);
    expect(store.byPath.get('C:/work/alpha')).toMatchObject({ error: null });
    expect(store.scanError).toBeNull();
  });

  /**
   * Cleared by the next attempt, so a stale cause does not sit under a button that has since
   * worked.
   */
  it('clears the previous failure when a launch succeeds', async () => {
    const store = useReposStore();
    store.Upsert(MakeStatus({ path: 'C:/work/alpha' }));
    store.SetOpenError('C:/work/alpha', 'could not run `code`');
    mockIPC(() => null);

    await OpenIn('C:/work/alpha', 'editor');

    expect(store.openErrors.has('C:/work/alpha')).toBe(false);
  });
});
