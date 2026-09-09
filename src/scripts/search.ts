/**
 * The repository search index.
 *
 * A module-scoped singleton, like `scan.ts`: there is one index per app, none of it is rendered
 * directly, and the rows it describes are the store's. What is rendered is the _result_, which the
 * table derives.
 *
 * # The corpus is live
 *
 * The pattern this borrows from indexes a prebuilt document set fetched once. Here the corpus is
 * the repository set: it streams in tier by tier and changes under a watcher, so the index is
 * maintained incrementally — `add` for a row that is new, `replace` for one that changed — and
 * never rebuilt. {@link IndexRows} does both, deciding per row, because the same batch can carry
 * either.
 *
 * Only cheap, stable fields are indexed: name, path, branch and upstream. **Never Tier 2.** Its
 * counts are lazy and mostly unknown, so indexing them would mean reindexing on every expand for no
 * searchable value.
 *
 * # `shallowRef`, and the version counter beside it
 *
 * The index is a large nested structure and deep reactivity over it is a performance disaster, so
 * the instance is a `shallowRef` — which means mutating it notifies nothing. That is the right
 * trade, but a table whose rows are still streaming does need to re-run its search as the corpus
 * grows. {@link indexVersion} is what it depends on instead: one number, bumped on every mutation,
 * cheap to track.
 */
import MiniSearch from 'minisearch';
import { ref, shallowRef } from 'vue';
import { isRead, type RepoRow } from '@/stores/repos';

/// Type
/**
 * What the index holds per repository. Its `path` is the id.
 */
type RepoDocument = {
  /**
   * Absolute path — the store's key, and what a match resolves back to.
   */
  path: string;
  /**
   * The folder name.
   */
  name: string;
  /**
   * The current branch, or `''` for anything else.
   */
  branch: string;
  /**
   * The upstream ref, or `''` when there is none.
   */
  upstream: string;
};

/// Data
/**
 * The shortest query worth running.
 *
 * One character matches most of a tree with `prefix` on, which is noise rather than a result.
 */
export const MIN_QUERY_LENGTH = 2;

/**
 * Bumped whenever the index changes, so a `computed` can depend on the corpus without depending on
 * the index itself. See the note above.
 */
export const indexVersion = ref(0);

/**
 * The index. `shallowRef` and never `ref`.
 */
const index = shallowRef(build());

/// Methods
/**
 * A fresh index, configured.
 *
 * `fields` is what gets searched and `storeFields` is what comes back; setting the second is what
 * makes a result flat rather than something to dig through.
 *
 * Fuzziness is length-conditional. Repository names are full of short fragments — `api`, `db`,
 * `ui`, `ssg` — so anything under five characters is matched exactly or not at all, and only longer
 * terms get the tolerance that makes a typo forgivable.
 *
 * @returns An empty index.
 */
function build(): MiniSearch<RepoDocument> {
  return new MiniSearch<RepoDocument>({
    idField: 'path',
    fields: ['name', 'path', 'branch', 'upstream'],
    storeFields: ['path'],
    searchOptions: {
      prefix: true,
      fuzzy: (term) => (term.length >= 5 ? 0.2 : false),
      // A repository is found by its name far more often than by anything else about it.
      boost: { name: 3, path: 1 },
    },
  });
}

/**
 * Adds or updates rows in the index.
 *
 * Guarded per row rather than per batch: `add` throws on an id it already holds and `replace`
 * throws on one it does not, and both cases are normal here — the `subscribe` snapshot repeats rows
 * the scan already sent, and a rescan repeats every row it sent last time.
 *
 * @param rows - The rows to index, in whatever state they are in.
 */
export function IndexRows(rows: RepoRow[]): void {
  for (const row of rows) {
    const document = describe(row);
    if (index.value.has(document.path)) index.value.replace(document);
    else index.value.add(document);
  }
  indexVersion.value += 1;
}

/**
 * Drops rows from the index.
 *
 * `discard` rather than `remove`: it marks the id and lets auto-vacuum reclaim, where `remove`
 * needs the exact document that was added and throws if what it is given differs.
 *
 * @param paths - The paths to drop.
 */
export function UnindexPaths(paths: string[]): void {
  for (const path of paths) {
    if (index.value.has(path)) index.value.discard(path);
  }
  indexVersion.value += 1;
}

/**
 * Empties the index, for a scan that starts over.
 *
 * A new instance rather than `removeAll`, because there is nothing to preserve and this cannot
 * leave discarded ids behind.
 */
export function ClearIndex(): void {
  index.value = build();
  indexVersion.value += 1;
}

/**
 * The paths matching `query`, or `null` when there is no query to run.
 *
 * `null` and not an empty set: "everything, unfiltered" and "nothing matched" are different
 * answers, and an empty set for the first would blank the table.
 *
 * AND first, then OR. Every term matching is what a user means by typing two words; falling back to
 * any term matching is better than showing nothing when they meant one of them.
 *
 * @param query - What the user typed.
 * @param _version - The current {@link indexVersion}. Deliberately unused, and required for exactly
 *   that reason: the index is a `shallowRef` whose mutations notify nothing, so a `computed`
 *   calling this has to read the version to depend on the corpus. Taking it as an argument puts
 *   that dependency at the call site where it can be seen, rather than hiding it in here.
 * @returns Matching paths, or `null` for no query.
 */
export function searchPaths(query: string, _version: number): Set<string> | null {
  const trimmed = query.trim();
  if (trimmed.length < MIN_QUERY_LENGTH) return null;

  let hits = index.value.search(trimmed, { combineWith: 'AND' });
  if (hits.length === 0) hits = index.value.search(trimmed, { combineWith: 'OR' });

  return new Set(hits.map((hit) => String(hit.id)));
}

/**
 * The document for one row.
 *
 * A row discovery has found but nothing has read has a name and a path and no refs, so it is
 * searchable by both from the moment it appears — and gets its branch when Tier 0 replaces it.
 *
 * @param row - The row to describe.
 * @returns Its document.
 */
function describe(row: RepoRow): RepoDocument {
  if (!isRead(row)) {
    return { path: row.path, name: row.name, branch: '', upstream: '' };
  }

  return {
    path: row.path,
    name: row.name,
    branch: row.head.kind === 'branch' ? row.head.name : '',
    upstream: row.upstream ?? '',
  };
}
