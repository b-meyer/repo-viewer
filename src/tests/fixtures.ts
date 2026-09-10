/**
 * Row builders for tests.
 *
 * Shared harness rather than per-test literals: a `RepoStatus` has eighteen fields, and
 * hand-writing them in every component test is how one of them quietly acquires a `0` where the
 * production wire would have sent `null`. Every tiered field defaults to `null` here for exactly
 * that reason — a test that wants a computed value says so.
 */
import type { DiscoveredRepo } from '@/scripts/generated/DiscoveredRepo';
import type { FileCounts } from '@/scripts/generated/FileCounts';
import type { RepoStatus } from '@/scripts/generated/RepoStatus';
import type { ScanTotals } from '@/scripts/generated/ScanTotals';

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
    // Equal to `gitDir` for everything but a linked worktree. The frontend reads neither — Rust
    // owns every path decision — so this is here to satisfy the generated type, which is exactly
    // the reminder that the field belongs to the watcher and not to the table.
    commonDir: 'C:/work/alpha/.git',
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

/**
 * What a finished scan reported, tier by tier.
 *
 * @param overrides - Fields to replace.
 * @returns A `ScanTotals`.
 */
export function MakeTotals(overrides: Partial<ScanTotals> = {}): ScanTotals {
  return {
    reposRead: 1,
    errors: [],
    elapsedMs: 900,
    discoveryMs: 40,
    tier0Ms: 130,
    tier1Ms: 700,
    ...overrides,
  };
}

/**
 * A Tier 2 read: four column totals.
 *
 * Not a partition of paths — a staged-then-modified file counts in two columns — so these are
 * deliberately not chosen to look like they add up to anything.
 *
 * @param overrides - Fields to replace.
 * @returns A `FileCounts`.
 */
export function MakeCounts(overrides: Partial<FileCounts> = {}): FileCounts {
  return { staged: 1, unstaged: 2, untracked: 3, conflicted: 0, ...overrides };
}
