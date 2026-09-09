import { describe, expect, it } from 'vite-plus/test';
import {
  AHEAD_BEHIND_CAP,
  NO_VALUE,
  fetchStaleness,
  formatAge,
  formatCount,
  formatFetchAge,
  shortId,
} from '@/scripts/utils';

const NOW = 1_700_000_000_000;
const MINUTE = 60 * 1000;
const DAY = 24 * 60 * MINUTE;

describe('utils', () => {
  /**
   * The mirror of `assert_eq!(AHEAD_BEHIND_CAP, 1000)` in `crates/repo-scan/tests/tier0.rs`.
   * `ts-rs` exports types but not constants, so the cap is duplicated; pinning the literal on both
   * sides means moving it fails two tests rather than silently rendering `1000` as an exact count
   * on one side of the wire.
   */
  it('pins the ahead/behind cap to the value Rust uses', () => {
    expect(AHEAD_BEHIND_CAP).toBe(1000);
  });

  describe('formatCount', () => {
    it('renders an unknown count as a dash and never as zero', () => {
      expect(formatCount(null)).toBe(NO_VALUE);
      expect(formatCount(null)).not.toBe('0');
    });

    it('distinguishes a real zero from an unknown', () => {
      expect(formatCount(0)).toBe('0');
    });

    it('marks a capped count as a lower bound', () => {
      expect(formatCount(AHEAD_BEHIND_CAP)).toBe('1000+');
      expect(formatCount(AHEAD_BEHIND_CAP + 5)).toBe('1000+');
      expect(formatCount(999)).toBe('999');
    });
  });

  describe('formatAge', () => {
    it('renders a null timestamp as a dash', () => {
      expect(formatAge(null, NOW)).toBe(NO_VALUE);
    });

    it('scales through the units', () => {
      expect(formatAge(NOW - 5_000, NOW)).toBe('5s');
      expect(formatAge(NOW - 12 * MINUTE, NOW)).toBe('12m');
      expect(formatAge(NOW - 5 * 60 * MINUTE, NOW)).toBe('5h');
      expect(formatAge(NOW - 3 * DAY, NOW)).toBe('3d');
      expect(formatAge(NOW - 21 * DAY, NOW)).toBe('3w');
    });

    /**
     * A clock skew that puts a commit in the future must not render a negative age.
     */
    it('clamps a future timestamp to zero rather than going negative', () => {
      expect(formatAge(NOW + 10_000, NOW)).toBe('0s');
    });
  });

  describe('fetchStaleness', () => {
    it('separates never-fetched from merely old', () => {
      expect(fetchStaleness(null, NOW)).toBe('never');
      expect(fetchStaleness(NOW - MINUTE, NOW)).toBe('fresh');
      expect(fetchStaleness(NOW - 10 * DAY, NOW)).toBe('stale');
      expect(fetchStaleness(NOW - 40 * DAY, NOW)).toBe('very-stale');
    });
  });

  describe('formatFetchAge', () => {
    it('names the never-fetched case instead of showing a dash', () => {
      expect(formatFetchAge(null, NOW)).toBe('never fetched');
      expect(formatFetchAge(NOW - 3 * DAY, NOW)).toBe('fetched 3d ago');
    });
  });

  describe('shortId', () => {
    it('abbreviates to the length git uses', () => {
      expect(shortId('0123456789abcdef')).toBe('0123456');
    });
  });
});
