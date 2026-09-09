/**
 * Row builders for tests.
 *
 * Shared harness rather than per-test literals: a `RepoStatus` has eighteen fields, and
 * hand-writing them in every component test is how one of them quietly acquires a `0` where the
 * production wire would have sent `null`. Every tiered field defaults to `null` here for exactly
 * that reason — a test that wants a computed value says so.
 */
import type { DiscoveredRepo } from '@/scripts/generated/DiscoveredRepo';
import type { RepoStatus } from '@/scripts/generated/RepoStatus';

/**
 * A repository the walk found but nothing has read.
 *
 * @param overrides - Fields to replace.
 * @returns A `DiscoveredRepo`.
 */
export function MakeDiscovered(overrides: Partial<DiscoveredRepo> = {}): DiscoveredRepo {
  return {
    path: 'C:/work/alpha',
    name: 'alpha',
    parent: 'C:/work',
    kind: 'normal',
    gitDir: 'C:/work/alpha/.git',
    ...overrides,
  };
}

/**
 * A repository Tier 0 has read. Every later tier is unknown, which is what a real Phase 3 row is.
 *
 * @param overrides - Fields to replace.
 * @returns A `RepoStatus`.
 */
export function MakeStatus(overrides: Partial<RepoStatus> = {}): RepoStatus {
  return {
    path: 'C:/work/alpha',
    name: 'alpha',
    parent: 'C:/work',
    kind: 'normal',
    head: { kind: 'branch', name: 'main' },
    upstream: null,
    ahead: null,
    behind: null,
    lastCommit: null,
    stashCount: 0,
    state: 'clean',
    lastFetchedMs: null,
    dirty: null,
    conflicted: null,
    counts: null,
    submodules: null,
    scannedAtMs: 1_700_000_000_000,
    error: null,
    ...overrides,
  };
}
