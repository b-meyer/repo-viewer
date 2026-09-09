/**
 * Formatters for values that may not exist.
 *
 * **Every function here takes a nullable and returns a `string`. None returns a number.** That is
 * deliberate and load-bearing: there is no `countOr(value, 0)` and no defaulting helper anywhere in
 * `src/`, so an uncomputed tier cannot be coerced into arithmetic or rendered as `0` through any
 * sanctioned path. Showing `0` for "not counted yet" is the most common bug in this class of app,
 * and the absence of a `T | null -> T` function is what makes writing it require going out of your
 * way.
 */

/**
 * The revision walk's cap. A count equal to this means "at least this many", not "exactly this".
 *
 * Mirrors `AHEAD_BEHIND_CAP` in `crates/repo-scan/src/status/ahead_behind.rs`. `ts-rs` exports
 * types but not constants, so this is duplicated rather than generated; both sides assert the
 * literal — Rust in `crates/repo-scan/tests/tier0.rs`, TypeScript in `utils.test.ts` — so moving
 * either one fails two tests.
 */
export const AHEAD_BEHIND_CAP = 1000;

/**
 * A fetch older than this is shown as stale.
 */
export const STALE_FETCH_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * A fetch older than this is shown as badly stale.
 */
export const VERY_STALE_FETCH_MS = 30 * 24 * 60 * 60 * 1000;

/**
 * What a row's tiered field renders as when there is no value.
 */
export const NO_VALUE = '—';

/**
 * Renders an epoch-millisecond timestamp as a compact age.
 *
 * @param ms - When it happened, or `null` when it never did.
 * @param now - The current time, passed in rather than read, so rendering is pure and testable with
 *   a frozen clock.
 * @returns Something like `4s`, `12m`, `3d`, `5w`, or {@link NO_VALUE}.
 */
export function formatAge(ms: number | null, now: number): string {
  if (ms === null) return NO_VALUE;

  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 60) return `${seconds}s`;

  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;

  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h`;

  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d`;

  const weeks = Math.round(days / 7);
  if (weeks < 52) return `${weeks}w`;

  return `${Math.round(weeks / 52)}y`;
}

/**
 * How stale the last fetch is.
 *
 * Ahead/behind is measured against `refs/remotes/*`, which is only as fresh as the last fetch, so
 * this is what decides how loudly the counts beside it are qualified.
 *
 * @param ms - `FETCH_HEAD`'s modification time, or `null` if the repo was never fetched.
 * @param now - The current time.
 * @returns Which band the age falls in.
 */
export function fetchStaleness(
  ms: number | null,
  now: number,
): 'never' | 'fresh' | 'stale' | 'very-stale' {
  if (ms === null) return 'never';

  const age = now - ms;
  if (age >= VERY_STALE_FETCH_MS) return 'very-stale';
  if (age >= STALE_FETCH_MS) return 'stale';
  return 'fresh';
}

/**
 * Renders the last-fetched age, naming the never-fetched case explicitly.
 *
 * `never fetched` rather than a dash: a repository that has never been fetched has ahead/behind
 * counts measured against nothing, and that is the single most misleading state the app can show.
 *
 * @param ms - `FETCH_HEAD`'s modification time, or `null`.
 * @param now - The current time.
 * @returns `never fetched` or `fetched 3d ago`.
 */
export function formatFetchAge(ms: number | null, now: number): string {
  return ms === null ? 'never fetched' : `fetched ${formatAge(ms, now)} ago`;
}

/**
 * Renders a count that may be uncomputed or capped.
 *
 * @param count - The count, or `null` when there is no answer.
 * @returns {@link NO_VALUE} For `null`, `1000+` at the cap, the number otherwise. Never `0` for an
 *   absent value — a genuine zero and an unknown are different facts.
 */
export function formatCount(count: number | null): string {
  if (count === null) return NO_VALUE;
  return count >= AHEAD_BEHIND_CAP ? `${AHEAD_BEHIND_CAP}+` : String(count);
}

/**
 * Shortens a hex object id for display.
 *
 * @param hex - The full object id.
 * @returns Its first seven characters, the length git itself abbreviates to.
 */
export function shortId(hex: string): string {
  return hex.slice(0, 7);
}
