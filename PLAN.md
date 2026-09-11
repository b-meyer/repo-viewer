# repo-viewer

A cross-platform desktop app: point it at a folder and get a live dashboard of every Git repo
beneath it — branch, ahead/behind, dirty state, file counts — without opening each one in an IDE.

Status: **Phase 4 complete — every tier is in.** Point it at a folder and rows stream into a table
as the walk finds them, then fill in tier by tier: branch, upstream, ahead/behind, stash count,
in-progress state, tip commit and last-fetched age from refs alone, then the dirty flag and
conflicted count from the worktree. Expanding a row reads its full per-file counts and submodule
list on demand and keeps them when it is collapsed; the scan summary reports each tier's cost
separately, and a repository that cannot be read says so as its batch is read rather than at the
end. Scans are cancellable, roots are managed in-app, and the whole gate is green — `vp check`,
`vp run typecheck`, `vp test run`, `vp run rust`, `vp run types`, and `vp run build`. Phases 5–7
make it pleasant: filters and search, watching, fetch.

---

## 1. What this is

### 1.1 Two goals

1. **The tool.** A multi-repo Git dashboard for personal use, shared with colleagues who
   want it.
2. **A delivery pattern.** Establish whether Vue/web skill transfers to shipping a desktop app to
   a smaller client — a site that cannot run a web server, or has connectivity too poor to rely
   on, but needs an intranet-style app against a **local SQL database**.

Goal 2 drives most of what follows. It is why the frontend stack matches the sibling dashboard app rather
than being chosen fresh, and why the code is split so the Tauri-specific parts stay thin. It is
also why repo-viewer is a good first subject: it exercises the whole Tauri↔Vue boundary while
having no database, isolating the mechanics from data-access concerns. The appendix records what
changes once SQL is added.

### 1.2 Scope for v1

**Read-only, plus batch fetch.** The app reports state and launches external tools; it does not
stage, commit, or push. Fetch is included because ahead/behind is meaningless without it (§8.2).

In scope: recursive discovery, tiered status, filtering and search, live refresh, opt-in fetch,
open-in-editor/terminal/file-manager, Windows installer.

Out of scope: mutating Git operations, mobile, multi-window, remote/API repos.

---

## 2. Architecture

### 2.1 Responsibility split

```
Vue 3 + Pinia (src/) ............ presentation only
  - repo table: sort, filter, group, search
  - filter chips: dirty | unpushed | detached | conflicted | stale-fetch
  - per-repo detail drawer (triggers Tier 2 on open)
  - scan progress, staleness badges, action buttons
  - NO git logic, NO path manipulation, NO filesystem access

IPC (@tauri-apps/api) ...........
  invoke()             -> commands (scan, cancel, refresh, fetch, open, pick root)
  Channel<ScanEvent>   -> one per scan: streamed results and progress
  Channel<RepoEvent>   -> one per app session, opened at startup: watcher, poll, and
                          fetch-driven row updates. No emit()/listen().

Rust (crates/ + src-tauri/) ..... all system work
  - parallel discovery
  - tiered Git reads
  - canonical row state: one map of path -> RepoStatus, merged per tier (§6.3)
  - filesystem watching and debounce
  - `git` CLI subprocess for fetch — the one thing this app writes — and the configured
    editor/terminal for `open_in`
  - path normalization, editor/terminal/file-manager launch
  - plugin calls (dialog, opener, store) — the frontend never imports a plugin package
  - settings and cache persistence: the roots, the `open_in` commands, the `watch` and `fetch`
    settings, the view state, and a snapshot of both row maps, all through `persist.rs` (§6.3)
```

The boundary is structural, not aspirational: `src/` cannot import the engine, because one is
TypeScript and the other a Rust crate.

### 2.2 The scan is tiered

Most of what the dashboard shows costs almost nothing; only file counts are expensive. Three
tiers stream independently, so a row appears before any worktree is touched.

| Tier                | Cost per repo                                                                                                                                                                                                  | Yields                                                                                                                        | When                                                                     |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| **0 — refs only**   | ~3 ms measured (§11); reads `.git/HEAD`, `packed-refs`, loose refs, `refs/stash` reflog, `FETCH_HEAD` mtime, revwalk with commit-graph                                                                         | branch, ahead/behind, upstream, stash count, state flags (rebase/merge/bisect/revert/detached), last commit, last-fetched age | immediately, every scan                                                  |
| **1 — dirty flag**  | ~17 ms measured warm, ~146 ms cold (§11) — an early exit on the first status item, **untracked and staged changes both included**; the conflicted count comes from index stage entries and touches no worktree | clean/dirty boolean, conflicted count                                                                                         | streams in right after Tier 0                                            |
| **2 — full counts** | ~35 ms measured warm (§11) — HEAD-tree↔index **and** index↔worktree, drained rather than early-exited, so both halves can report the same path                                                                 | staged / unstaged / untracked / conflicted, and the submodule list                                                            | lazily: expanded rows, explicit refresh — never in the default scan path |

Tier 0 alone answers "which repos have unpushed commits?" with zero worktree I/O. This tiering
matters more to perceived performance than any library choice, and the measured gap is why: Tier 1
costs about **24x** Tier 0 on a cold cache (§11). Waiting to paint a complete row would mean
holding an answer that is ready in a third of a second behind one that takes another seven.

### 2.3 Diagrams

Two views live outside this document so they sit where they are used:

- **Layers and libraries** — which library runs at which layer — is in
  [README.md](./README.md), as orientation for anyone opening the repo.
- **A scan end to end** — the tiered sequence — is in [AGENTS.md](./AGENTS.md), where it belongs
  with the invariant that Tier 0 stays refs-only.

---

## 3. Stack

### 3.1 Rust backend (`crates/`, `src-tauri/`)

| Crate                            | Version           | Role                                                                       |
| -------------------------------- | ----------------- | -------------------------------------------------------------------------- |
| `tauri` / `tauri-build`          | 2.11.5 / 2.6.3    | shell, windowing, IPC / `src-tauri` build script                           |
| `gix`                            | 0.87.1            | all Git reads — **default features only**, no network/TLS features (§10.3) |
| `ignore`                         | 0.4.33            | parallel repo discovery                                                    |
| `rayon`                          | 1.12.0            | per-repo fan-out                                                           |
| `notify`                         | 8.2.0             | filesystem watching                                                        |
| `notify-debouncer-full`          | 0.7.0             | debounce — mandatory (§7.3)                                                |
| `tokio`                          | 1.53.1            | async runtime for Tauri commands                                           |
| `serde` / `serde_json`           | 1.0.229 / 1.0.151 | IPC payloads                                                               |
| `thiserror`                      | 2.0.20            | engine errors                                                              |
| `anyhow`                         | 1.0.104           | `src-tauri` layer only                                                     |
| `tracing` / `tracing-subscriber` | 0.1.44 / 0.3.23   | structured diagnostics — the subscriber is installed by `run()`            |
| `ts-rs`                          | 12.0.1            | TypeScript type generation (§4.2)                                          |
| `dunce`                          | 1.0.5             | strips the `\\?\` prefix `canonicalize()` returns on Windows (§5.2)        |
| `tempfile`                       | 3.27.0            | dev-only, test fixtures                                                    |
| `tauri-plugin-dialog`            | 2.7.3             | native folder picker                                                       |
| `tauri-plugin-store`             | 2.4.4             | settings and the row cache (§6.3)                                          |
| `tauri-plugin-opener`            | 2.5.5             | reveal in Explorer/Finder, open in editor                                  |
| `tauri-plugin-window-state`      | 2.4.1             | window geometry                                                            |

**`gix`, not `git2`, on the read path.** The app's premise is scanning hundreds of repos and
feeling instant, and libgit2 is the slower option per repository: `git_status_list_new` runs
~2.5× slower than `git status` without the untracked cache and ~5–6× slower with it
([libgit2#4230](https://github.com/libgit2/libgit2/issues/4230)), ~3 s vs ~0.1 s on the `rust`
repo ([exa#28](https://github.com/ogham/exa/issues/28)). It also has no early-exit, so "is this
repo dirty?" costs a full diff. `gix`'s status platform is a lazy iterator, so the dirty flag is
"take the first item", and `gix-credentials` invokes real `git credential` helpers.

Three `gix` traps and their handling:

- **`Repository::is_dirty()` is not the dirty flag.** Its docs state "untracked files do _not_
  affect this flag" — it sets `dirwalk_options = None`, so a repo with a brand-new file reports
  clean.
- **`into_index_worktree_iter()` is not the iterator either**, which is the less obvious half. It
  sets `head_tree = None`, so it compares only the index against the worktree: a repository with
  staged-but-uncommitted changes and a clean worktree reports clean through it. Tier 1 uses
  `repo.status(Discard)?.untracked_files(Collapsed).should_interrupt_owned(..).into_iter(..)` and
  takes the first item. `into_iter` keeps the HEAD-tree comparison `status()` sets up by default
  and runs all three checks at once — the directory walk for untracked files, index-to-worktree for
  unstaged changes, tree-to-index for staged ones — so any item at all means dirty. Note the
  `should_interrupt_owned`: `gix` accepts an owned `Arc` or a `&'static` flag here, not the plain
  reference Tier 0 takes — and it must be a **private** flag rather than the scan's shared one,
  because `gix` writes to whatever it is given. `status::private_interrupt` makes one;
  [AGENTS.md](./AGENTS.md) carries the failure it prevents.
- **A conflicted path has up to three index entries.** A merge conflict writes stages 1, 2 and 3
  for one path, so counting entries with a non-zero stage triples the number of conflicted files.
  The count is of distinct paths, which is what `git status` shows.
- **`with_boundary` is not `^upstream`.** Its docs say a boundary "is distinctly different from
  exclusive revspecs" — it stops the walk at the given ids but does not hide their ancestors, so
  any merged history overcounts. Ahead/behind is `rev_walk([local]).with_hidden([upstream])`
  counted, then swapped. `with_hidden` warns that disjoint histories may traverse everything, so
  the walk is capped (1000, shown as "1000+"). Cost depends on a commit-graph being present;
  without one every commit is an object decode, and §11 measures that at roughly **10×**. The seam
  is `crates/repo-scan/src/status/ahead_behind.rs` — the only file naming `rev_walk` — and there
  is no backend trait. Two settled choices there: `Sorting::BreadthFirst`, because a count needs
  no ordering and the time-based sortings decode a commit time per commit; and never
  `first_parent_only`, which would disagree with the `git rev-list` oracle the tests assert
  against. Equal tips short-circuit to `(0, 0)` with no walk at all, which on a real tree is the
  common case and where most of the budget is saved.
- **Pre-1.0, breaks on minor bumps** ([gix#470](https://github.com/GitoxideLabs/gitoxide/issues/470)).
  Pin exact and treat upgrades as tasks.

**In-process reads, not subprocess.** Windows process creation is >20× slower than Linux and
acutely sensitive to Defender and corporate AV
([benchmark](https://www.bitsnbites.eu/benchmarking-os-primitives/)). Spawning 300 `git status`
processes on the primary dev machine is the worst available option. Fetch is the exception
(§8.2) — it is network-bound, so process cost is noise.

**Parallel discovery uses `ignore`, not `walkdir`.** `walkdir` is a sequential iterator; rayon
can parallelize work on the entries it yields but not the directory _descent_, which is the
bottleneck. `ignore::WalkBuilder::build_parallel()` (the crate behind ripgrep) gives a genuinely
parallel walk with a prune predicate. `jwalk` 0.9.0 is the fallback if `ignore`'s gitignore
machinery gets in the way.

**Storage is JSON, not SQLite.** Hundreds of small flat records, no relational queries, no
history, no concurrent writers. `tauri-plugin-store`, driven from Rust, covers it. Revisit only
if commit-history or time-series features land.

**Plugins are called from Rust only.** `dialog`, `opener`, and `store` are registered in the
builder and reached through the app's own commands (`pick_root`, `open_in`, settings). The
frontend installs none of the `@tauri-apps/plugin-*` packages, so `src-tauri/capabilities/` is
`core:default` and nothing else. Plugin scopes are enforced only on webview-initiated calls, so
the path check in `open_in` (§6.1) is the real guard, not a capability scope.

### 3.2 Frontend (`src/`)

The sibling dashboard app stack at latest versions. Greenfield has no migration cost, so this repo runs
ahead of that catalog and serves as the proving ground for bumps later applied there.

| Package                                | Version                                  | Notes                                                                          |
| -------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------ |
| `vite-plus`                            | 0.3.0                                    | the toolchain: Vite + Vitest + oxlint + oxfmt + task runner, one pinned bundle |
| `vite`                                 | `npm:@voidzero-dev/vite-plus-core@0.3.0` | is Vite 8.2.2 — i.e. plain-Vite latest                                         |
| `vitest`                               | **4.1.11 — not 5.0.0**                   | lockstep (§3.3)                                                                |
| `vue`                                  | 3.5.42                                   | latest stable; 3.6 is at rc.7                                                  |
| `vue-router`                           | 5.3.1                                    | file-based routing via `vue-router/vite`                                       |
| `pinia`                                | 4.0.3                                    | ESM-only                                                                       |
| `@vue/devtools-api`                    | 8.2.1                                    | required peer of pinia 4 (`^8.1.5`); pinia does not bundle it                  |
| `reka-ui`                              | 2.10.4                                   | headless primitives, wrapped as `App*`                                         |
| `@vueuse/core`                         | 14.4.0                                   |                                                                                |
| `@vitejs/plugin-vue`                   | 6.0.8                                    |                                                                                |
| `tailwindcss` + `@tailwindcss/vite`    | 4.3.3                                    | CSS-first; custom palette, `--spacing: 1px`                                    |
| `typescript`                           | **6.0.3 — not 7.0.2**                    | see §3.3                                                                       |
| `vue-tsc`                              | 3.3.11                                   |                                                                                |
| `minisearch`                           | 7.2.0                                    | repo search (§8.3)                                                             |
| `@iconify-json/bi` + `bootstrap-icons` | 1.2.7 / 1.13.1                           |                                                                                |
| `@vue/test-utils` / `jsdom`            | 2.5.0 / 30.0.1                           |                                                                                |
| `@tauri-apps/api`                      | 2.11.1                                   | the only Tauri package in the frontend                                         |
| `@tauri-apps/cli`                      | 2.11.4                                   |                                                                                |

`oxlint` 1.79.0, `oxfmt` 0.64.0, `oxlint-tsgolint` 7.0.2001 and `tsdown` 0.22.14 arrive inside
`vite-plus` — do not add them as catalog entries. No `@tauri-apps/plugin-*` package and no schema
validator: plugins are reached through Rust commands (§3.1), and every value the frontend receives
_about a repository_ is typed end to end by `ts-rs` (§4.2), which names the two exceptions and what
stands in for a type where there is none.

**Every version exact, declared once** in the root `pnpm-workspace.yaml` `catalog:` block.
Catalogs are a pnpm-workspace feature, so that file exists even though this is a single package.
Bump with `vp update -L <pkg>`, never by widening a range.

**Virtualization is deferred.** 500 rows of simple DOM does not need it, and it costs real
complexity in measurement, sticky headers, keyboard nav, and find-in-page. Measure, and add
`@tanstack/vue-virtual` 3.13.36 past a measured threshold (~1000 rows, or a p95 frame-budget
miss).

**Rows are not fixed height, and whoever virtualizes them must use dynamic measurement.** The
original plan here was to keep the markup fixed-height; the Upstream-and-sync cell makes that
impossible, and deliberately so. It renders one line for a repository with no upstream and three
for one that has counts and a fetch age, because the counts are meaningless without that age
beside them (§8.2) — collapsing it to a uniform height would mean either dropping the age or
padding every short row with a blank line. `@tanstack/vue-virtual` supports `measureElement` for
exactly this; budget for it rather than assuming a constant `estimateSize`.

### 3.3 Version constraints

Three pins are load-bearing and two documented Tauri config values are wrong on Vite 8. All of
it is operational rather than architectural, so it lives in [AGENTS.md](./AGENTS.md) under
**Hard rules** and **Durable failure shapes**: `typescript` at 6.0.3, `vitest` at 4.1.11, the
`vite`/`vite-plus`/`vitest` lockstep, `build.minify: 'oxc'`, `envPrefix: 'TAURI_ENV_'`, and
the `NODE_ENV` + `--mode production` pairing.

The TypeScript pin has an exit condition. `typescript@7.0.2` is `latest`, but `vue-tsc` needs
the compiler's programmatic API, which TypeScript 7.0 does not expose stably; 7.1 is slated to
ship it, and `vue-tsgo` bridges the gap in the meantime. The pin holds until 7.1 is released
**and** `vue-tsc` runs on it; review at the 7.1 release, not before.

---

## 4. Structural decisions

### 4.1 The workspace, and four rules it imposes

The engine is a separate crate so `cargo test` and timing runs work without booting a webview,
and so the §2.1 boundary is compile-enforced. `tauri dev` watches `src-tauri` _and its dependent
workspace crates_, so editing `repo-scan` still triggers a rebuild.

1. **`[profile.release]` must live in the root `Cargo.toml`.** Cargo ignores profile sections in
   member crates with only a warning, so the `lto`/`codegen-units = 1`/`strip` block belongs at
   the root or the binary ships unoptimized and large.
2. **`target/` is at the workspace root**, so `/target/` is ignored from the root `.gitignore` and
   there is no `src-tauri/.gitignore`. A `/target/` entry scoped to `src-tauri/` matches nothing
   once the workspace moves the directory, which is how build output ends up staged.
3. **Keep `[lib] name = "..._lib"`** in `src-tauri/Cargo.toml`. The suffix prevents a lib/bin
   name collision **on Windows specifically**
   ([cargo#8519](https://github.com/rust-lang/cargo/issues/8519)).
4. **`panic = "unwind"` (the default) and `opt-level = 3`.** Tauri's app-size guide suggests
   `panic = "abort"` and `opt-level = "s"`; both are wrong here. Per-repo work runs under
   `catch_unwind` so a `gix` panic on one corrupt repo becomes `RepoStatus.error` instead of
   killing the app — `abort` defeats that, and rayon propagates worker panics. A scanning engine
   wants speed over a few hundred KB of binary.

### 4.2 Rust ↔ TypeScript type sync

Generate the types. Hand-maintained mirrors of `RepoStatus` drift, and the bug drift produces is
precisely the one §8.1 exists to prevent: an uncomputed tier rendering as `0` instead of unknown.

`ts-rs` 12.0.1 behind an optional `typescript` feature on `repo-scan`, deriving `TS` on the
`model.rs` types. `cargo test --features typescript` writes `src/scripts/generated/`, which is
**committed** so the frontend builds without a Rust toolchain. CI regenerates and fails on a
diff, which is what actually prevents drift.

`tauri-specta` would additionally type the command bindings but couples the engine to Tauri and
has roughly a tenth of the adoption. The handful of command signatures in `src/scripts/ipc.ts`
are cheap to hand-write and rarely change.

**Two shapes are hand-mirrored, and both exceptions are narrow on purpose.** `OpenTarget` is three
string variants, mirrored as a union in `ipc.ts` beside the signature that takes it; a mismatch
fails immediately and loudly, because Rust refuses to deserialise anything else. The view state
(§6.1's `ui_settings`) is the larger one: it is vocabulary of the table, Rust keeps it without
reading inside it, and typing it in Rust would mean either putting `FilterChip` and `SortKey` in
the engine crate — the crate the appendix swaps out for a different domain — or a `ts-rs` derive in
`src-tauri`, which would put that crate's whole dependency tree behind `vp run types`. So
`src/scripts/settings.ts` owns it, and pays for owning it: the settings file is hand-editable,
nothing validates it on the way in, and `parseUiSettings` therefore narrows field by field with a
default for each. Do not widen either exception to anything a tier writes — that is what the
paragraph above is about.

**`model.rs` types are `gix`-free and `ts-rs`-expressible.** Object ids are hex `String`, not
`gix::ObjectId`; timestamps are `u64` epoch milliseconds, not `SystemTime` (`ts-rs` has no impl
for it, and serde would emit a `secs`/`nanos` struct). Every type carries
`#[serde(rename_all = "camelCase")]` and enums with data are internally tagged
(`#[serde(tag = "kind")]`) so the generated TypeScript is a discriminated union. `ts-rs` mirrors
serde attributes, so the wire shape and the types agree by construction.

### 4.3 Tests and fixtures

Tests sit next to their subject (`RepoTable.vue` + `RepoTable.test.ts`); `src/tests/` holds only
shared harness and setup.

A nested `.git` cannot be committed inside the outer repo, and the cases most worth testing — a
linked worktree, a submodule, a bare repo (§5.2) — are exactly the ones needing real git
metadata. So `tests/support/fixtures.rs` builds the tree at test time into a `tempfile::TempDir`
by shelling out to `git`. No fixture is committed and none needs to be: the builder is the
fixture. It carries two `git` invocation requirements that are not obvious and are recorded in
[AGENTS.md](./AGENTS.md) — `protocol.file.allow=always`, without which a local-path submodule is
refused, and a `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` override so the developer's own config
cannot change the shape of the tree.

There are **three** trees, and they are separate on purpose. `discover.rs` asserts exact
`repos_found` and `dirs_visited` counts, so adding a repository to the discovery tree would make
every one of those counts a maintenance tax on tests that do not care about it. And the fetch
tree cannot be shared at all, for a different reason: a fetch **mutates** the repository it runs
in, so one shared instance would have tests moving each other's ahead/behind — intermittently,
because cargo runs a binary's tests in parallel.

- `build()` — the discovery tree: all four repository kinds, a pruned directory, a nested checkout.
- `status_tree()` — the status tree, shared by all three tiers: a bare upstream and clones off it
  for every ahead/behind topology (in sync, ahead, behind, diverged, a merge from upstream, a
  criss-cross with two merge bases, no upstream, and a deleted tracking ref), every parked
  operation, an unborn HEAD, a HEAD detached onto an annotated tag object, stash entries, a bare
  repo, a linked worktree, and the worktree shapes Tiers 1 and 2 read. Built once per test binary
  behind a `OnceLock`, because eighty-odd `git` spawns is not something to repeat per test on
  Windows. It is safe to extend, which is the reason for the split above: nothing asserts a total
  over it, and the tier suites iterate discovery's output against oracles.
- `fetch_tree()` — a bare origin and whatever clones one test makes of it. Built **per test**, not
  behind a `OnceLock`, for the mutation reason above; a bare `init` and a local clone is cheap
  enough that isolation is the obvious trade. It carries the helpers a fetch test needs to stage
  a case: advance the origin, push and delete a branch, clone bare, make a repository with no
  remote, and point one at a dead remote.

Ahead/behind is asserted against `git rev-list --left-right --count HEAD...@{upstream}` rather
than against hand-counted numbers. That is the point: a literal encodes what the author believed
the topology was, and the topologies worth testing are exactly the ones where that belief is
unreliable — while git's answer cannot drift from git's behaviour. The literals are kept _as well_,
so an implementation returning `(0, 0)` everywhere cannot pass by agreeing with an oracle that
also reads zero.

Tier 1's fixtures follow the same principle, and there are four rather than one because a single
"dirty repository" would let two of three plausible mistakes pass: `untracked/` (one new file, never
added) catches a flag built on `is_dirty()`, `stagedonly/` (added, worktree clean) catches one built
on `into_index_worktree_iter()`, `unstaged/` is the case everyone gets right and is there so an
always-`true` flag cannot pass by agreeing with the other two, and `conflicted/` is a modify/modify
merge over two files — six index entries — so a count of entries reads 6 where a count of paths
reads 2. The oracle is `git status --porcelain`, run over every repository in the tree.

Tier 2 needs more fixtures again, because it reports four numbers where Tier 1 reports a boolean:
`counts/` has one change of each kind so no two columns can be swapped undetected, `intentadd/` is
`git add -N`, `stagedthenmodified/` is the `MM` case that proves the counts are per-column rather
than a partition of paths, `renamed/` is a staged rename that must count once, `untrackeddir/` is a
directory of three untracked files that collapses to one entry, and `submodparent/` against
`submodnone/` separates "read, and there are none" from "not read". Its oracle parses porcelain's
**two status columns** into the four counts, and must read the output untrimmed — the leading space
is the index column, and trimming moves an unstaged change into the staged count. That oracle is
what settled where an intent-to-add path belongs, rather than the author's belief about it.

Frontend code that reaches `ipc.ts` is tested with `mockIPC` from `@tauri-apps/api/mocks`,
which intercepts `invoke` and `Channel` traffic without a webview; no hand-rolled mocks. Under
`mockIPC` nothing is serialised, so the handler is given the **live** `Channel` instance rather
than its `"__CHANNEL__:id"` wire form — which is what makes streaming testable at all.
`src/tests/channel.ts` drives one; its `index` must start at 0 and rise by exactly 1, because
`Channel` buffers an out-of-order message in a private field and delivers nothing until the gap
fills, presenting as a hung assertion rather than an error.

**There are no `#[tauri::command]`-level Rust tests, deliberately.** They need
`tauri = { features = ["test"] }`, which pulls the test harness into the dependency graph of a
shipped binary. The commands are thin — validate an input, clone an `Arc`, spawn — and everything
underneath them is covered by the `state`, `stream` and `pipeline` module tests. `pipeline`'s run
against a **real** `tauri::ipc::Channel`, because `Channel::new` needs no `AppHandle` or `Webview`;
its handler collects the serialised body, so the assertions run over what actually crosses the wire.
That is also why there is no `EventSink` trait: the real channel removed the reason for one.

**No `criterion`.** Its repeated-sampling model is the wrong shape for a whole-tree scan — 100
samples of a multi-second operation is a five-minute run fought with `sample_size`. A tier's timing
is one recorded number, taken with `std::time::Instant` in the examples below. Reach for
`criterion` only for micro-level pieces if they profile hot; `ahead_behind` takes its cap as a
parameter, so it is already callable in isolation on a single repository.

### 4.4 Running the engine without the GUI

Two examples, and Cargo compiles `examples/` during `cargo test`, so neither can rot. Both need no
extra crate, manifest, or `clap`. Promote to `crates/repo-scan-cli/` with `clap` only once one of
them wants subcommands and flags.

- **`scan.rs`** — `cargo run --release --example scan -- C:/Working [--rows]`. Discovery and
  Tier 0 over a real tree, each timed separately, with the commit-graph population printed beside
  the timing because a number recorded without it cannot be interpreted. `--rows` prints one line
  per repository, which is the "debug one repo without a webview in the way" case.
- **`synth.rs`** — `cargo run --release --example synth -- <dir> [count] [depth]`. Generates a tree
  of diverged repositories, for the two measurements a real tree cannot give: scale beyond what is
  on the machine, and a with/without commit-graph pair. Because every repository in it is
  generated, writing commit-graphs into them is unobjectionable in a way that writing into the
  user's own repositories is not. It disables git's auto-maintenance for the reason recorded in
  [AGENTS.md](./AGENTS.md), and refuses to write into a directory that already exists.

### 4.5 How this maps onto the sibling dashboard app

| Sibling dashboard app                                       | repo-viewer                                     |
| ----------------------------------------------------------- | ----------------------------------------------- |
| `apps/ui` — Vue SPA                                         | `src/` — same stack, same conventions           |
| OData over HTTP, `credentials: 'include'`                   | `invoke()` + `Channel<T>` (§6)                  |
| `apps/api` — Functions handlers, `ok()` / `errorResponse()` | `src-tauri/src/commands/` — `#[tauri::command]` |
| `packages/db` — data access via `mssql`                     | `crates/repo-scan/` — the domain engine         |
| `packages/shared` — contracts, zod                          | `ts-rs`-generated types (§4.2)                  |
| SWA auth, `allowedRoles`                                    | none — the OS user is the user                  |
| Azure Static Web App                                        | an installer (§9)                               |

---

## 5. Repo discovery

### 5.1 Algorithm

Parallel walk from each configured root via `ignore::WalkBuilder`:

- `git_ignore(false)`, `hidden(false)`, `standard_filters(false)` — this walk must _see_ ignored
  and hidden directories in order to prune them; it is not a content search.
- `filter_entry` prunes by name, only names that are near-certainly generated: `node_modules`,
  `target`, `.venv`, `venv`, `__pycache__`, `.gradle`, `.terraform`, `Pods`, `.next`, `.nuxt`.
  User-extensible. `bin`, `obj`, `build`, `dist` and `vendor` are deliberately absent — a Go
  `vendor/` tree or a `Source/build/` folder legitimately holds repos, and a name-pruned repo
  vanishes silently. The scan summary reports how many directories were pruned so the omission
  is visible.
- `.git` itself is never descended — `hidden(false)` makes it visible to the walk, so the prune
  predicate has to exclude it explicitly.
- `same_file_system(true)` by default — avoids network shares and mounted volumes, a common
  cause of multi-minute scans.
- `follow_links(false)` by default — symlink loops are real.
- `max_depth` configurable, default 8.
- `threads(n)` set explicitly. The walker and the rayon pool each default to the full core count,
  which doubles the thread population during a scan.
- On finding a repo, record it and **stop descending** by default (config flag to continue). A
  bare repo is never descended into either way — it _is_ a Git directory, so everything below it
  is object storage.
- The `submodules` list on a parent row comes from the parent's config, never from the walk, so
  the two can never disagree. Reading it is Tier 2 work rather than Tier 0: `.gitmodules` is a
  worktree file, and when it is absent the lookup falls back to parsing the whole index. Whether a
  submodule also gets a row of its own is exactly what the descend flag decides.

An unspecified `threads` means **half** the core count, not all of it: from Tier 0 onwards the
walk and the rayon pool run at the same time, so taking both defaults would put twice as many
threads on the machine as it has cores.

Discovery yields `DiscoveredRepo` — path, name, parent, kind, and the resolved Git and common
directories — rather than a partial `RepoStatus`. The §8.1 Tier 0 fields are not `Option`, because a row that
has been read always has them, and discovery has read nothing; a "partial" row would have to lie.
This is what the `ReposFound` event carries, and Tier 0 turns it into a `RepoStatus`. `ScanOpts` and
`ScanSummary` cross the same boundary and live in `model.rs` beside it.

The Git directory is resolved once, here: `<path>/.git` for a normal repo, `<path>` itself when
bare, and the private directory the `.git` _file_ names for a worktree or submodule. Everything
downstream consumes it rather than re-resolving — Tier 0 opens it, and §7.2's watch set is
registered against it.

The **common** directory is resolved here too, and for the same reason. It is where `refs/`,
`logs/` and `FETCH_HEAD` actually live, which for a linked worktree is not `git_dir` — so the watch
set spans both, and Tier 0's fetch age has to read `Repository::common_dir()` rather than
`git_dir()`. Read through `gix::discover::path::from_plain_file_relative_to_file` against the
`commondir` file, which is the function `is_git` itself uses; every other kind has no such file and
so falls through to `git_dir`, which for a submodule is exactly the absence that identified it.

### 5.2 Cases that must be handled

These are what make naive implementations wrong:

The first three are `gix::discover::is_git`'s contract rather than something reimplemented here.
It follows a `.git` _file_ to the directory it names and then requires a valid HEAD, an `objects/`
and a `refs/`, and it separates a worktree from a submodule by whether a `commondir` sits beside
the private Git directory — more robust than matching `worktrees/` or `modules/` in a path.

- **`.git` is often a file, not a directory.** Linked worktrees and submodules write a `.git`
  _file_ containing `gitdir: <path>`. Testing `path.join(".git").is_dir()` silently misses both.
  Test for existence, then resolve.
- **Bare repos** have no `.git` — detect via `HEAD` + `objects/` + `refs/` at the root. List
  them flagged, with no worktree status.
- **Linked worktrees** share one object store. Each is its own row with its own HEAD and index,
  but must not be double-counted as separate repos.
- **Windows long paths cut the other way.** Rust's `std::fs` converts long absolute paths to the
  `\\?\` form on its own, so deep `node_modules` trees do not break the walk. The trap is
  `canonicalize()`, which _returns_ verbatim `\\?\C:\...` paths: they render badly, confuse
  `git` CLI arguments, and compare unequal to the user-typed form. Canonicalize through `dunce`
  (§3.1), which drops the prefix whenever the path is representable without it. Junctions and
  reparse points are already reported as symlinks by `std`, so `follow_links(false)` covers them.
- **Case sensitivity.** macOS is case-insensitive-preserving, Linux sensitive, Windows
  insensitive. Canonicalize for deduplication, display the on-disk form.
- **Permission errors** are collected and surfaced as a scan summary, never fatal to a scan.

---

## 6. IPC

### 6.1 Commands

```rust
subscribe(on_event: Channel<RepoEvent>) -> Vec<RepoStatus>  // once at startup; lives for the
                                                  //   session. Returns the canonical map, so a
                                                  //   reloaded webview repaints without rescanning
scan_roots(roots: Vec<PathBuf>, opts: ScanOpts, on_event: Channel<ScanEvent>) -> ScanId
cancel_scan(id: ScanId)
refresh_repo(path: PathBuf, tier: Tier) -> RepoStatus  // tiers 0..=tier; also pushes the row
full_status(path: PathBuf) -> RepoStatus          // Tier 2, on demand; the merged row
fetch_repos(paths: Vec<PathBuf>, on_event: Channel<FetchEvent>) -> FetchId   // git CLI
cancel_fetch(id: FetchId)
git_info() -> Option<GitInfo>                     // what the startup probe found
open_in(path: PathBuf, target: OpenTarget)        // editor | terminal | file manager
pick_root() -> Option<PathBuf>                    // native folder dialog, via the plugin's Rust API
add_root(path) -> Vec<PathBuf>                    // canonicalises, validates, persists; returns the list
remove_root(path) -> Vec<PathBuf>                 // evicts its rows, pushed as RepoEvent::Removed
list_roots() -> Vec<PathBuf>
ui_settings() -> JsonValue                        // the persisted view state, opaque to Rust
save_ui_settings(ui: JsonValue)
```

The root commands return the new list rather than `()`, so the frontend mirrors it exactly as it
mirrors the rows and never maintains a second copy.

Commands registered via `invoke_handler` are callable by all windows by default and need no
capability declaration. Because the plugins are reached only from Rust (§3.1), no plugin
permission is declared either: `src-tauri/capabilities/default.json` grants `core:default` and
nothing else. Filesystem work inside our own commands is not constrained by any plugin scope —
the Rust side is trusted — which is why all fs access stays in Rust and `tauri-plugin-fs` is not
used. The corollary is that commands validate their own inputs: a command taking a repository path
accepts only a key of one of Rust's two maps (§6.3), never an arbitrary string from the webview.

**Which map depends on what the command needs.** `full_status` requires a row to merge Tier 2 into,
so it checks the rows. `refresh_repo` and `open_in` check the **superset**: the map of what discovery
found, which `src-tauri/src/state.rs` keeps beside the rows because `RepoStatus` carries no
`git_dir` and every engine entry point needs the resolved one. That map holds repositories whose HEAD
could not be read and which therefore have no row. For `refresh_repo` that is exactly the case worth
retrying — `merge_tier0` already inserts where there was nothing, so the §8.1 total-failure grade is
recoverable without rescanning the tree. For `open_in` it is sharper still: a repository that will
not open is the one a user most needs to go and look at, and refusing to reveal it in the file
manager would be the wrong reading of this rule. Both maps are keyed identically, so neither check is
looser than the row map's for anything that does have a row.

`open_in`'s file-manager target is the opener plugin's `reveal_item_in_dir`. Its other two are not:
neither has a platform API to ask for, and on Windows the default association for a _folder_ is
Explorer, so "open with the system default" would open the file manager three times over. They run a
command from the settings file instead, which makes `open_in` the second place in the app that spawns
a process after §8.2's fetch — with the flag inverted, since a terminal must keep the console that a
fetch must suppress.

`ui_settings` / `save_ui_settings` carry the chips, the sort and the grouping. Rust keeps that object
without reading inside it, and it is the one value crossing this boundary that `ts-rs` does not
generate — see the `persist.rs` invariant in [AGENTS.md](./AGENTS.md) for why, and for what that
costs.

`full_status` returns the **merged row** rather than a payload of its own. `counts` and `submodules`
are fields of `RepoStatus` (§8.1) and Rust owns the canonical copy, so a separate shape carrying the
same two values would be a second source of truth for them — the thing §6.3 exists to prevent. It is
also what lets an expanded-then-collapsed row keep its counts with no cache on the frontend: the
store's mirror still holds them. A Tier 2 read that fails reports through this command's own `Err`
rather than onto `RepoStatus.error` — see §12 item 7.

Roots are the other half of that rule, and a different check: they are by definition _not_ map
keys. `scan_roots` accepts only a path already on the root list, and `add_root` is the single place
a new path enters the app — it canonicalises through the same helper discovery uses and requires
the path to be a directory. Canonicalising through that one helper is what makes a root a prefix of
the row keys beneath it, which is what `remove_root` relies on to evict them.

`fetch_repos` returns a `FetchId` and has a `cancel_fetch` beside it. At four concurrent with a
60-second deadline, three hundred repositories over a slow link is minutes of work, and a batch
with no way to stop it would be a worse gap than §6.4's documented per-repository one — that is
bounded by a single read. Starting a fetch does **not** cancel another, unlike `scan_roots`: a
scan supersedes the scan before it because both answer the same question about the same tree,
where two fetches are two sets of repositories a user asked for.

`git_info` is a command of its own rather than a field on `subscribe`'s reply. That command
returns `Vec<RepoStatus>` and is what the whole app hangs off; widening it to a struct would touch
every one of its tests to carry one optional value, where one small command per concern is the
shape `list_roots` and `ui_settings` already have.

`refresh_repo` is fallible for the §8.1 total-failure grade: a repository that will not open, or
whose HEAD is unreadable, has no honest `RepoStatus` to return. It reports that failure rather than
synthesising a row, and the existing row keeps its last-known values with its `scanned_at` age
showing — which is the same thing a scan does when one repository in a tree cannot be read.

### 6.2 `Channel<T>` for scan results, not events

Tauri's event system is explicitly "not designed for low latency or high throughput situations":
payloads are always JSON strings, unsuitable for larger messages. `tauri::ipc::Channel` is
"designed to be fast and deliver ordered data" and is what Tauri uses internally for streaming. A
scan emitting hundreds of tiered updates is the channel use case. `Channel` is `Clone + Send`, so
the one passed to `subscribe` is stored in app state and outlives the command; watcher, poll, and
fetch results all go out on it. `emit()`/`listen()` are not used at all — one mechanism, one
ordering.

Batch channel sends — flush every ~50 ms or every 25 repos — rather than one send per repo. The
per-message JSON serialization cost is what bites.

Every `ScanEvent` carries its `ScanId`, so a rescan or a root change mid-scan cannot interleave
stale rows with fresh ones.

**The id cannot be the primary filter, though, and a design that assumes it is has a hole.** Rust
starts the pipeline before `scan_roots`'s reply crosses back, so a batch can arrive while the
frontend still does not know the id to compare it against. The frontend therefore keys acceptance
on a **generation counter** captured in the event handler's closure _before_ the invoke — which has
no window at all — and treats the id as the guard on top: `activeId` latches from the first
accepted event, and once latched, any event bearing a different id is dropped. The resolved id
cross-checks the latch rather than establishing it.

### 6.3 Rust owns the canonical state

A tiered stream has a merge problem: a Tier 0 result for a repo arriving after its Tier 1 result
carries `dirty: None`, and a naive "replace the row" would erase a value the UI already shows.
The merge therefore happens once, in Rust. `src-tauri/src/state.rs` holds
`HashMap<PathBuf, RepoStatus>`, and the **full merged row** is what goes over the channel — on the
scan's `Channel<ScanEvent>` for a scan result, on the session `Channel<RepoEvent>` for a watcher,
poll, or fetch push (§6.2). The Pinia store is a mirror keyed by path
— it never merges, never infers, and never holds a value Rust does not. So there is exactly one
source of truth on each side of the IPC boundary.

**The cache serialises both of Rust's maps**, not just the rows. `RepoStatus` carries no `git_dir`
and every engine entry point needs the resolved one, so a cache of rows alone would restore a table
whose every row refused to refresh or expand until a scan had rediscovered it. `persist.rs` writes
the pair and `AppState::restore` seeds both, once, before any command can run — which is what makes
plain insertion correct there and a merge unnecessary.

**The rule is tier ownership, not field-wise option preference.** Each tier replaces every field
it owns, `None` included, and leaves fields owned by other tiers untouched. The distinction is
load-bearing and easy to get backwards: `upstream`, `ahead`, `behind`, `last_commit` and
`last_fetched_ms` are `Option` because the _answer_ can be none — no upstream configured, never
fetched — not because the value might be uncomputed. A merge preferring `Some` over `None` on
every `Option` would report a deleted upstream as live for the rest of the session, which is
§8.1's dishonesty pointing the other way. Only the genuinely tiered fields — `dirty`,
`conflicted`, `counts`, `submodules` — mean "not computed yet" when `None`, and only those survive
a merge from another tier.

**There is one deliberate exception, and it is a separate operation rather than a softening of the
rule.** A watcher refresh re-reads Tiers 0 and 1, which by tier ownership leaves `counts` and
`submodules` exactly as they were — so a repository that just changed would show a pre-change count
beside a post-change branch, and the drawer shows those counts with **no age beside them**, so the
pair reads as one freshly measured moment. `AppState::invalidate_tier2` nulls them first, which is
the same carve-out the cache load path already makes and for the same reason. It is called by the
watcher only: a poll tick has no evidence anything changed, and `refresh_repo` was asked for the
tiers it named.

That obliges the frontend, because "counting…" must be transient (§8.1) and Rust cannot discharge
the obligation — which rows are expanded is window state Rust neither knows nor should.
`src/scripts/detail.ts` re-reads Tier 2 for an expanded row whose counts have gone, which is the
same guard chain an expand already uses.

### 6.4 Cancellation

Every scan gets an `Arc<AtomicBool>`. Discovery checks it and returns `WalkState::Quit`; `gix`
status calls take it via `should_interrupt`; the rayon fan-out checks it between repos.
`cancel_scan` flips it, a root change flips the previous scan's flag before starting the next,
and window close flips all of them. The scan body runs on `tauri::async_runtime::spawn_blocking`
so the Tauri runtime's threads are never occupied by a walk.

**A per-repository read is not cancellable, and that is a known gap rather than an oversight.**
`full_status`, `refresh_repo`, a watcher refresh and a poll pass each make a private flag that
nothing ever flips, so collapsing a drawer part-way through a large repository does not stop the
read — the flag is there because `gix` requires one. Fixing it means a registry keyed by
repository, and the flags handed out of it must stay **private** per walk for the
`should_interrupt_owned` reason in [AGENTS.md](./AGENTS.md). What bounds the damage today is that
these reads are single-repository and short; the whole-tree work is the part that can be cancelled.

Window close is the exception that is handled: it flips every scan flag and drops the watcher, which
stops the debouncer's thread and — by dropping the sender its callback holds — ends the refresh
thread too, while the runtime is still up.

---

## 7. Live updates

### 7.1 One watcher, many paths

`notify` spawns a thread per `Watcher` object, so 300 watchers means 300 threads. There is **one**,
created by `notify_debouncer_full::new_debouncer` — which owns a `RecommendedWatcher` underneath,
so reaching for `recommended_watcher()` directly would mean either a second watcher or an
undebounced one. `RepoWatcher::sync` then calls `watch()` once per **path**, and a repository
contributes three of them (§7.2), so the count that matters against a platform's limit is paths and
not repositories: **903 paths for 301 repositories**, measured.

`Debouncer`'s `Drop` stops its thread, so the handle lives on `AppState` for the session. A watcher
owned by the function that built it stops watching the moment that function returns, and nothing
fails when it does.

### 7.2 What to watch

Three watches per repo, all inside the git dir — five for a linked worktree, and fewer for a
repository with no commits. The set is built by `crates/repo-scan/src/watch/set.rs` and **filtered
by existence**, because `notify`'s `watch()` fails with `PathNotFound` and `git init` does not write
`logs/` until the first ref update.

- the git dir root, **non-recursive** — catches `HEAD`, `index`, `packed-refs`, `FETCH_HEAD`,
  `ORIG_HEAD`, `MERGE_HEAD`, and the `index.lock` bursts (§7.3)
- `refs/`, **recursive** — a non-recursive watch sees only direct children, so it misses every
  slash-named branch (`refs/heads/feature/x`) and every remote-tracking update
  (`refs/remotes/origin/main`), which is exactly the ahead/behind signal. The refs tree is a few
  dozen small files, so recursion here is free.
- `logs/HEAD` — appended on every commit, checkout, and reset regardless of branch name

A linked worktree has a private git dir (`.git/worktrees/<name>/`, its own `HEAD` and `index`)
and a shared common dir (refs, objects); both are watched, resolved through `gitdir:` and
`commondir` — which discovery already did, onto `DiscoveredRepo.common_dir`. **Both `refs/` trees
are watched, not just the common one**: the per-worktree refs (`refs/bisect/*`, `refs/worktree/*`)
live in the private directory, so a bisect running in that worktree is visible nowhere else.

The common dir is shared with the repository the worktree was linked from, so one watched path can
belong to two rows. `watch/mod.rs`'s reverse index maps a path to **every** repository that asked
for it — both are reported for a write there, because they share remotes and a remote-tracking
update moves both their ahead/behind counts — and that list is also the reference count, so a
worktree going away cannot release the refs its parent is still watching.

The worktree itself is never watched: recursively watching worktrees means watching
`node_modules`, which is how tools burn CPU and blow past inotify limits. Worktree edits are
picked up by the poll, by refresh-on-focus, or by the `index` change that follows any `git add`.

### 7.3 Debouncing is mandatory

Git does not write `.git/index` once; it writes `index.lock`, writes, then renames, so a single
`git add` produces a create/modify/remove burst. `notify-debouncer-full` with a ~300–500 ms
window plus a per-repo refresh cooldown is required to avoid a refresh storm, and the two are not
the same guard: the debouncer collapses one operation's burst, while `live.rs`'s `COOLDOWN`
collapses separate operations — `add`, then `commit`, then `push` — arriving a second apart.

Never let a watcher callback block: if it stalls, OS events pile up in the kernel buffer and are
silently dropped on overflow. Everything the callback in `live::start` does is one `mpsc` send; the
work belongs to the thread draining it, which batches through the same `stream::next_batch` the
scan pipeline uses.

### 7.4 Watching is an optimization, never the source of truth

- **Linux:** inotify has a per-user watch limit; exceeding it surfaces as "No space left on
  device". `watch/mod.rs` matches `notify::ErrorKind::MaxFilesWatch` — the kind, never the message,
  which is what the backend maps `ENOSPC` onto — appends
  `sysctl fs.inotify.max_user_watches=524288`, and sends it as `RepoEvent::WatchFailed` so it
  reaches the window rather than only the log.
- **macOS:** FSEvents cannot observe files the process does not own; Docker on Apple Silicon
  returns `os error 38`.
- **All platforms:** `notify`'s own docs warn it "may fail to receive all events" at high file
  counts, and that backends are "not a 100% reliable source".

So always ship a low-frequency poll (Tier 0 every ~60 s, configurable) and a refresh-on-focus. Both
run on one thread parked on `recv_timeout`: the timeout is the poll and a message is the focus
refresh, which is what makes §7.5's "the poll and the focus handler take the same path" true of the
code rather than only of the design.

**`PollWatcher` is not the escape hatch for a network mount or a container; `watch.enabled: false`
is.** Swapping the backend would keep the watcher's shape while making it walk the tree itself,
which is the poll again with a worse interface — where turning the watcher off leaves the Tier 0
poll already there and running. The setting exists for exactly that case and drops to slower
updates rather than to none.

The poll claims **Tier 0 and nothing else**, which is tier ownership applied to triggers: it has
evidence that time passed, not that anything changed. The watcher has the latter, so it re-reads
Tier 1 as well and drops Tier 2 — see §6.3.

### 7.5 Refresh happens in Rust

A debounced event never crosses the IPC boundary as a "something changed" notice. The watcher's
refresh thread re-runs Tier 0 and Tier 1 for that repo through `live::refresh_one`, merges each
tier into the canonical map (§6.3), and pushes the merged row on the session channel. The frontend
has nothing to do but render what arrives.

`refresh_one` has exactly three callers — that thread, the `refresh_repo` command, and the fetch
driver — so a user-requested refresh, a watcher-driven one and a post-fetch one cannot disagree
about what a refresh is. The **poll** is the fourth trigger and deliberately does not use it: a
whole-tree pass wants `read_tier0_all_with`'s rayon fan-out rather than three hundred sequential
opens. What all four share is that Rust owns the read and the merge, and that a full row is what
goes out — not one function. The focus handler (`WindowEvent::Focused(true)`) is not a fifth: it
sends a tick to the poll's own thread, so it _is_ the poll, run early.

**A fetch owns the refresh of what it touched, and suppresses the watcher for it.** A fetch writes
`FETCH_HEAD` and `refs/remotes/*`, both of which are in the §7.2 watch set, so without this every
fetch is refreshed twice — once by the fetch and once by the watcher noticing the fetch's own
writes. The suppression is a **deadline** rather than set membership, because a path left behind by
a fetch that panicked would stop that repository updating for the session, silently. It **defers**
a notice rather than dropping one: a fetch writes nothing Tier 2 measures, so a fetch alone does
not invalidate, but the notice it suppressed might have been the user's own `git add` — so the
entry records that one arrived and Tier 2 is invalidated on release. The poll is gated
whole-pass on a fetch being in flight, symmetrically with a scan.

The group is the repository **and every sibling sharing its common directory**: fetching a linked
worktree moves its parent's ahead/behind, measured rather than assumed.

---

## 8. Data, fetch, and search

### 8.1 Model

```rust
#[serde(rename_all = "camelCase")]
struct RepoStatus {
    path: PathBuf,
    name: String,
    parent: PathBuf,             // for group-by-folder; the frontend does no path splitting
    kind: RepoKind,              // Normal | Bare | LinkedWorktree | Submodule

    // Tier 0
    head: Head,                  // Branch { name } | Detached { id: String } | Unborn — tagged
    upstream: Option<String>,    // Some with ahead/behind None = configured, no tracking ref
    ahead: Option<u32>,          // None = not computable; == the cap means "at least the cap"
    behind: Option<u32>,
    last_commit: Option<CommitSummary>,
    stash_count: u32,
    state: RepoState,            // Clean | Merging | Rebasing | Bisecting | CherryPicking
                                 //   | Reverting
    last_fetched_ms: Option<u64>,  // mtime of FETCH_HEAD; None if never fetched (§8.2)

    // Tier 1
    dirty: Option<bool>,         // None = not yet computed; true includes untracked files
    conflicted: Option<u32>,     // index entries with stage > 0; None = not yet computed

    // Tier 2
    counts: Option<FileCounts>,  // staged, unstaged, untracked, conflicted
    submodules: Option<Vec<SubmoduleStatus>>,  // names+paths+ids all Tier 2; see below

    scanned_at_ms: u64,
    error: Option<String>,       // per-repo failure, never fatal to the scan
}
```

`Option` on every tiered field is load-bearing: the UI must render a partial row honestly
("counting…") rather than showing `0` for "unknown". That is the most common bug in this class of
app. Times are `u64` milliseconds and ids are hex strings for the reasons in §4.2.

The Tier 0 fields are **not** `Option`, which is why discovery has a type of its own —
`DiscoveredRepo`, with `ScanOpts` and `ScanSummary` beside it (§5.1). A row that has been read
always has a head and a state; one that has not been read cannot produce them, and a partial
`RepoStatus` would have to invent them.

That splits per-repository failure into two grades, and the split follows from the sentence above
rather than being an extra rule:

- **Total** — the repository will not open, or its HEAD is unreadable. No `RepoStatus` exists,
  because there is no honest `head` to give one. The row stays a `DiscoveredRepo`, and the cause
  reaches the frontend twice over: on `ScanEvent::RepoErrors` as the batch that failed is read, and
  again in `ScanTotals.errors` when the scan ends, so a webview that reloaded mid-scan still has it.
  The per-batch delivery is what lets the row render as `unreadable` rather than `counting…` for the
  rest of the scan. Do not reach for a `Head::Unknown` variant to paper over this: the absence of a
  row _is_ the signal, and a variant would make every consumer handle a state that means "ignore
  everything else here".
- **Partial** — the repository was read, but its upstream, ahead/behind, or stash count failed.
  The row survives with those fields `None` and the cause on `RepoStatus.error`. Losing one field
  is not worth losing the row.

`stash_count` is the one non-`Option` field that can be wrong: it has to report a number, so a
failed read reports `0` and sets `error`. A count is only trustworthy on a row without an error.

Persisted to the JSON store so launch paints last-known state immediately, then reconciles: Rust
restores both maps into state before any command can run, so `subscribe` returns the cached rows and
no new command is needed to show them, and the launch scan then overwrites each row as it re-reads it
— **without** clearing them first, which would make the window flash empty for the length of a scan.
Rows the completed scan does not find are evicted, which is what removes a repository deleted between
sessions.

Every cached row renders with its `scanned_at` age until refreshed, and nothing on the load path
touches that age. `counts` and `submodules` are the exception and are dropped on load: the drawer
shows them with no age beside it, so a cached count would read as freshly measured.

### 8.2 Ahead/behind is relative to the last fetch

Ahead/behind is measured against the local remote-tracking ref (`refs/remotes/origin/*`), which
is only as fresh as the last `git fetch`. An app whose selling point is "what haven't I pushed?"
is misleading if it shows stale numbers silently. So:

- Show a per-repo **"last fetched"** timestamp and visually degrade rows past a threshold. It is
  the mtime of `FETCH_HEAD` — of **whichever** of the git directory and the common directory is
  newer, because git writes it wherever the fetch ran. See the trap in
  [AGENTS.md](./AGENTS.md); reading only the common directory reports "never fetched" for a
  linked worktree seconds after fetching it.
- Fetching is explicit, opt-in, and bounded — never part of a scan, and never on launch. Fetching
  300 repos unprompted is hostile. Bounding it takes **two** mechanisms and neither subsumes the
  other: a concurrency cap bounds instantaneous load (four TLS handshakes, not three hundred), and
  a five-minute per-repository guard bounds _repeated_ load so that pressing the button twice does
  not fetch everything twice. The guard applies to a **batch only** — one row's button clicked
  twice is intent — and its clock is `FETCH_HEAD`'s mtime rather than a map, so a failed fetch is
  retried the moment credentials are fixed and a never-fetched repository is never skipped.
- **"Fetch all" means what the table is showing.** With a chip active, a button that ignored it and
  fetched three hundred repositories while the user was looking at twelve would misstate what one
  click does — and filter-then-fetch is the intended workflow.
- **Fetch via the `git` CLI.** Fetch is where credential helpers, SSH config (`~/.ssh/config`,
  agents, jump hosts), corporate proxies, and custom transports matter, and where getting it
  wrong means hanging on a credential prompt with no UI. This is also why `keyring` is not
  needed: delegate to the credential helper already installed (Git Credential Manager on
  Windows, Keychain on macOS).
- **Subprocess hygiene.** `GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never` and
  `-c credential.interactive=false` so a missing credential fails instead of waiting on a terminal
  or a popup that nobody is watching; `CREATE_NO_WINDOW` (`creation_flags(0x0800_0000)`) on Windows
  or every fetch flashes a console; a per-process timeout (~60 s); at most ~4 concurrent fetches;
  and `-c gc.auto=0 -c maintenance.auto=false`, because `git fetch` runs auto-maintenance exactly
  as `git commit` does and an unbounded repack would be charged to the deadline. The environment is
  **inherited whole** — clearing it would destroy the credential helpers and SSH config the CLI was
  chosen for.

  **None of those variables closes `ssh`'s own prompts**, and overriding `core.sshCommand` to add
  `-o BatchMode=yes` would stomp a user's jump-host configuration. The timeout is the only
  universal backstop, which is why it is not optional.

- **After each completion the row is re-read at Tier 1 through `live::refresh_one` and pushed on
  the session channel.** Not a `last_fetched_ms` patch: a fetch moves `behind`, can move `ahead`,
  moves the tracking ref's tip, and with `--prune` can remove `upstream` outright — so writing that
  one field would put a one-second-old age beside four stale numbers, which is this section's own
  premise inverted.

- **The flags are `--quiet --prune --no-recurse-submodules --all`.** `--prune` is required by the
  honesty rules rather than optional: without it a deleted remote branch leaves its tracking ref
  forever and `behind` counts against a ref that no longer exists upstream. Its destructive
  neighbour `--prune-tags` deletes the user's own local tags and is **never** passed. `--all`
  because `last_fetched_ms` is a repository-level fact, so fetching one remote of three and
  stamping the whole row fresh would overstate everything the row does not show.

- **Where a fetch failure goes: nowhere on the row.** See §12 item 8.

### 8.3 Search

`minisearch` 7.2.0. A sibling docs site runs it in production
(`src/composables/useSearch.ts`); the tuning below is borrowed from there. At a few hundred
short strings a `String.includes` filter in a `computed` would also do; MiniSearch is here for
parity with its pattern, not because the corpus needs it.

1. **`shallowRef`, never `ref`, for the instance.** The index is a large nested structure; deep
   reactivity over it is a performance disaster. The corollary is that mutating it notifies
   nothing, so `src/scripts/search.ts` exports an `indexVersion` counter beside it and bumps it on
   every change: that is what a `computed` depends on to re-run its search as rows stream in, and
   the page reads it at the call site rather than hiding the dependency inside the search.
2. **Module-scoped singleton.** One index, shared, in a module of plain functions — the same shape
   as `scan.ts` and for the same reason: nothing about the index is rendered. What _is_ rendered is
   the query, which is why it lives in the view store with the chips and the sort rather than here.
   The search is an inline filter over the table, so there is no open/closed state and no selected
   index to keep.
3. **`fields` vs `storeFields` are different lists.** `fields` is searched; `storeFields` is what
   comes back — set it and results are flat (`r.slug`), unset and you dig through `r.obj.*`.
4. **Length-conditional fuzziness:** `fuzzy: (term) => term.length >= 5 ? 0.2 : false`, plus
   `prefix: true`. Exact-only under 5 characters matters more here than in docs, since repo names
   are full of short fragments (`api`, `db`, `ui`, `ssg`).
5. **AND first, OR as fallback.** Search `combineWith: 'AND'`; if empty, re-run with `'OR'`.

Also carry `boost` for field weighting (repo name over its path), `MIN_QUERY_LENGTH = 2`, and
`results` as a `computed` over `query`.

**The corpus is live, not static.** That site fetches a prebuilt index once; here the corpus _is_ the
repo set, streaming in tier by tier and mutating on watcher events. So there is no `fetch` and no
build-time artifact: the index is fed from `scan.ts`, which is already the one file that turns
events into store writes, so the mirror and the index are updated from the same place and cannot
disagree. `add` for a row that is new and **`replace(doc)`** for one that changed — guarded per row
rather than per batch, because `add` throws on an id it holds and `replace` on one it does not, and
both cases are normal here: the `subscribe` snapshot repeats rows a scan already sent, and a rescan
repeats every row it sent last time. MiniSearch 7 has the full incremental surface — `add`, `addAll`,
`addAllAsync`, `remove`, `removeAll`, `replace`, `discard`, `discardAll`, `vacuum`, `has`,
`getStoredFields`, `search`, `autoSuggest`, plus `toJSON` / static `loadJSON`. Prefer `discard`
over `remove` and let auto-vacuum reclaim; do not rebuild on every change.

Index only stable, cheap fields — repo name, path, branch, upstream. Never Tier 2 counts: they
are lazy and mostly unknown, so indexing them means reindexing on every tier completion for no
search value.

Do not add an `optimizeDeps.include` entry for it. That site needs one because `minisearch` is
reached _through_ a library excluded from pre-bundling; here it is a direct dependency imported
from `src/`, so Vite's initial scan pre-bundles it with no configuration.

---

## 9. Packaging

Targets built: NSIS `.exe` and WiX `.msi` on Windows x64 (MSI is Windows-build-only); `.dmg` on
macOS; `.deb` and `.AppImage` on Linux. Windows produces **two** of each — a `downloadBootstrapper`
pair and an `offlineInstaller` pair.

`bundle.targets` is **"all"**, which selects the applicable targets per platform. An explicit
list is the trap: a Windows-only one silently produced no macOS or Linux installer at all, because
`tauri build` skips a target the host cannot make and still exits zero.

Not built, and what they would cost: ARM64 Windows needs
`rustup target add aarch64-pc-windows-msvc` plus the VS "C++ ARM64 build tools" component, and the
NSIS installer itself still runs x86 under emulation there even though the app would be native;
macOS universal builds use `universal-apple-darwin`. Neither has a known machine in the audience.

`.deb` is the primary Linux artifact and AppImage is best-effort — AppImage bundles its
dependencies but has recurring packaging bugs, e.g. a missing `libwebkit2gtkinjectedbundle.so` on
a pristine 22.04 build ([tauri#12463](https://github.com/tauri-apps/tauri/issues/12463)).

Config that must be set rather than left on defaults:

```jsonc
"app": {
  "security": {
    // default is null, i.e. no CSP. Tauri injects nonces for its own scripts.
    "csp": "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:"
  }
},
"bundle": {
  "macOS": { "minimumSystemVersion": "10.15" },   // config default is 10.13; real floor is Catalina
  "windows": {
    "webviewInstallMode": { "type": "downloadBootstrapper", "silent": true },
    // tauri.offline.conf.json overrides only this, for egress-blocked fleets. Both builds emit
    // the SAME filenames, so the second overwrites the first unless the first is staged away.
    "wix": { "upgradeCode": "…" },   // else derived from productName, and a rename breaks upgrades
    "nsis": { "installMode": "currentUser" }  // recorded and matched on when upgrading
  },
  "linux": { "deb": { "depends": ["libwebkit2gtk-4.1-0", "libgtk-3-0"] } }
}
```

No system tray is planned, which drops `libappindicator3-1` — one fewer Linux dependency. Keep it
that way unless a tray is wanted.

**Signing is what goal 1 still waits on, and it is measured rather than predicted.** Corporate-managed
Windows machines run application allowlisting — **ThreatLocker** is the lever, with Defender for
Endpoint and tamper protection resident beside it — and it denies _execution_ of
freshly written, low-prevalence executables outright (`os error 5` with correct ACLs), independent
of SmartScreen. On this machine the split is exact and reproducible: the binary under
`target/release/` runs, because that tree is allowlisted by path, and the identical binary installed
to `%LOCALAPPDATA%\Repo Viewer\` is denied. The installer completes; the app it installs will not
start.

So an unsigned installer handed to a colleague does not get a click-through, it gets blocked — and
the installer is not the only thing that has to clear. Tauri's NSIS installer extracts
`nsis_tauri_utils.dll` to `%TEMP%` and loads it for the `SemverCompare` its upgrade detection needs,
and that DLL is blocked on its own account.

The two workable paths are an Authenticode certificate trusted by the tenant, or an IT-issued allow
indicator by publisher or hash. Either is an IT request with lead time. **File it naming a
publisher rule, not just the certificate**: ThreatLocker allowlists by publisher, hash or path, so
a signed build nobody wrote a rule for is still blocked, and a hash rule has to be repeated per
build. macOS Gatekeeper and notarization matter only if a Mac build ships.

Signing is not configured in this repo. When the certificate lands the change is
`bundle.windows.certificateThumbprint`, `digestAlgorithm: "sha256"` and a `timestampUrl`, plus
wherever the key is allowed to live — which is its own question while the repository is public and
on a personal account. How colleagues receive versions is open decision 5.

**Installing a new version over an old one works, and two settings keep it working.** NSIS reads
`DisplayVersion` from `…\CurrentVersion\Uninstall\<ProductName>`, compares it against the incoming
build, and offers to remove the old version first — automatically under `/P`. `bundle.windows.wix.upgradeCode`
is pinned to a fixed GUID because Tauri otherwise derives it from `productName`, and MSI performs a
major upgrade only when that code is stable and the version increments: a later rename would
silently turn every upgrade into a second side-by-side install. `bundle.windows.nsis.installMode`
is set to `currentUser` explicitly for the same class of reason — the template records the mode and
matches on it, so a default that moves in a future Tauri release would break detection on machines
that already have the app.

---

## 10. Platform support

### 10.1 End-user requirements

Tauri bundles the frontend into the native binary, so **no Node and no Rust on an end-user
machine.** Windows and macOS need nothing extra; Linux needs one package-manager dependency and
has a hard version floor.

| Platform                                    | Webview            | Present out of the box?                            | End-user action                                               |
| ------------------------------------------- | ------------------ | -------------------------------------------------- | ------------------------------------------------------------- |
| Windows 11                                  | WebView2 Evergreen | **Yes — inbox**                                    | none                                                          |
| Windows 10 1803+ with Nov 2022 update       | WebView2 Evergreen | "The vast majority of Windows 10 devices have it"  | none in practice; the installer's bootstrapper covers the gap |
| Windows 10 LTSC / Server / clean images     | WebView2           | **Often not present**                              | bootstrapper (~2 MB) or ship `offlineInstaller`               |
| macOS 10.15+                                | WKWebView          | **Yes — part of the OS**                           | none                                                          |
| Ubuntu 22.04+, Debian 12+                   | webkit2gtk-4.1     | usually installed; declared as a `.deb` dependency | `apt` resolves it                                             |
| Ubuntu 20.04 and older, Debian 11 and older | —                  | **4.1 does not exist in those repos**              | **not a supported target**                                    |

**Windows is not unconditional.** Microsoft states the runtime "may be missing on clean Windows
10 installs, Windows Server, or LTSC editions" — all realistic in a managed enterprise fleet.
The installer's bootstrapper is the mechanism that closes that gap. As a diagnostic only, the
binary reads the `pv (REG_SZ)` value for the WebView2 Runtime under **both**
`HKEY_LOCAL_MACHINE` and `HKEY_CURRENT_USER` before creating the window, so a missing runtime
produces a native message box naming the fix rather than a blank exit — and it reads the HKLM value
through `WOW6432Node`, without which it finds nothing on a machine that has the runtime. Keep
`downloadBootstrapper` for general distribution and build a second `offlineInstaller` artifact for
locked-down environments. **That artifact is ~210 MB against the bootstrapper's ~4 MB**; Tauri
documents the difference as ~127 MB, and it is well over that. The exact size tracks the WebView2
runtime version the build downloads, so treat it as a band rather than a constant — a local build a
day earlier came out at 254 MB. The number matters because it goes in front of a user choosing which
link to click.

**Linux is version-gated, not just dependency-managed.** A Tauri v2 `.deb` declares
`libwebkit2gtk-4.1-0` and `libgtk-3-0`, so `apt` pulls them — but 4.1 exists in jammy 22.04,
noble 24.04, 25.10 and 26.04, and **not** in focal 20.04. Tauri v2 requires 4.1 specifically.
Declare Ubuntu 22.04 / Debian 12 as the floor and build Linux artifacts in a 22.04 container:
glibc compatibility is forward-only, so building on a newer system raises the minimum glibc and
produces binaries that fail on the stated floor.

### 10.2 External tools are real but graceful dependencies

Viewing status needs no Git — that is in-process `gix`. Fetch does (§8.2). Detect its absence at
startup and disable those actions with an explanation rather than failing at click time; the rest
of the app stays fully functional.

**Both layers, because the startup probe is an affordance and not the truth.** The same argument
this section makes below for the editor and the terminal — a `PATH` can change while the app runs
— applies to `git` no less, so `fetch_repos` resolves it again per invocation and refuses if it
has gone, and the engine reports `GitMissing` per repository if it disappears mid-pass. The probe
runs `git --version` rather than only resolving a path, because on a managed machine a `git.exe`
that exists and a `git.exe` this process may execute are different facts. And the explanation is
on the page as well as in the button's tooltip: a tooltip is unreachable by keyboard and invisible
to anyone who does not hover, which is not "with an explanation".

`open_in`'s editor and terminal are the same kind of dependency and degrade differently, on
purpose. What they run is configured rather than fixed (§6.1), so there is nothing to detect at
startup that would still be true at click time — a `PATH` can change and the file can be edited
while the app runs. They report the failure against the row whose button was pressed instead,
which is also the only honest answer for a command a user chose themselves. Revealing a folder in
the file manager depends on nothing.

### 10.3 No native dependencies

Beyond Tauri's unavoidable webview, this stack adds **no C library dependencies**, which is the
main portability payoff:

- **`gix` needs no network stack.** All of its networking features (`blocking-network-client`,
  `async-network-client`, `blocking-http-transport-curl`, `blocking-http-transport-reqwest`) are
  opt-in, not default. Because fetch is delegated to the `git` CLI, none are enabled — so no
  OpenSSL, no curl, no reqwest, and none of the per-platform TLS decisions that normally dominate
  cross-compiling a Rust desktop app.
- **`gix` compression is pure Rust.** gitoxide always uses `zlib-rs`, which benchmarked ~1%
  _faster_ than the C `zlib-ng` it replaced. The old `max-performance` / `max-performance-safe`
  split is obsolete: no performance-versus-purity trade, and no zlib to link.
- **Choosing `gix` over `git2` removes a C build.** `git2` with vendored libgit2 drags in
  libgit2, libssh2, and an OpenSSL decision per platform.
- **`ignore`, `rayon`, and `notify` are pure Rust** over native OS APIs — inotify, FSEvents,
  `ReadDirectoryChangesW`.

### 10.4 CI

**GitHub Actions**, because that is where the repository is — `github.com/b-meyer/repo-viewer`.
Azure DevOps is the house convention elsewhere and would work through a service connection and the
`GitHubRelease@1` task, but it puts the pipeline somewhere other than the code and buys nothing
here.

Two workflows. `.github/workflows/ci.yml` on push and PR to `main`; `.github/workflows/release.yml`
on a `v*` tag, with `permissions: contents: write`.

| Runner           | Builds                          | Notes                                           |
| ---------------- | ------------------------------- | ----------------------------------------------- |
| `windows-latest` | NSIS `.exe`, WiX `.msi`, x64    | MSI cannot be cross-built; the leg that matters |
| `macos-latest`   | `.dmg`                          | `minimumSystemVersion: 10.15` is set            |
| `ubuntu-22.04`   | `.deb`, `.AppImage` best-effort | **22.04, never `ubuntu-latest`** — glibc floor  |

Given the audience — a single developer plus a handful of colleagues — the Windows leg is the pipeline and
the other two are proof of portability. `fail-fast: false` is what keeps that distinction real: a
red Linux leg must not cancel the Windows one, and must not withhold a Windows release.

CI invokes the toolchain as `pnpm exec vp …` because `vp` is not global on a runner. That is more
load-bearing than it looks — `pnpm exec` is what puts `node_modules/.bin` on `PATH` for `vp` **and
everything it spawns**, and `tauri build`'s `beforeBuildCommand` shells out to `vp` again from
inside. Node comes from `actions/setup-node` with `node-version-file: '.node-version'`; pnpm from
`npm i -g pnpm@<pinned>`, after which pnpm enforces the `packageManager` field itself. Rust legs run
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` through
`vp run rust`, alongside the frontend's `vp check`. They also run `vp run types` and fail on a diff,
which is what actually prevents the committed bindings in `src/scripts/generated/` from drifting
from `model.rs` (§4.2), and `vp run versions`, which is what prevents a tag from publishing assets
that disagree with the version inside them.

`vp run smoke` is the per-platform launch check: it starts the built binary, requires it to still be
alive five seconds later, then stops it and requires it to go away. That is the honest ceiling for a
GUI binary observed from outside — a missing runtime, a bundle that did not build, a panic in
`run()` or a plugin that fails to register all kill the process inside that window. Linux runners
have no display, so the workflow puts `xvfb-run` in front of it. `vp run verify` asserts the built
bundle's `import.meta.env.PROD` flag (§3.3).

Releases are the distribution channel: the tag workflow attaches every installer to a GitHub Release
and a user downloads and runs it. Windows builds twice — the `downloadBootstrapper` pair, then the
`offlineInstaller` pair via `--config src-tauri/tauri.offline.conf.json`. **Both emit identical
filenames**, so the first pair is staged before the second build runs and the second is suffixed
`-offline`.

---

## 11. Roadmap

Each phase gets a runbook in `docs/` when it starts, written against the tree as it exists then,
and is deleted when the phase completes — durable facts move into README.md, AGENTS.md, and the
phase's own entry below, which is why `docs/` is empty or absent whenever no phase is open.
No phase is currently open, and every phase below is delivered.

**Phase 0 — Environment and structure.** The Cargo workspace with its root `[profile.release]`
(§4.1), `pnpm-workspace.yaml` with the catalog and the `vite`→core override, `vite.config.ts`
carrying lint/fmt/test/tasks, the two TypeScript configs, the engine crate with `model.rs`, the
`src-tauri` shell with its four Rust-driven plugins and a `ping` command, and the Vue shell.
TypeScript is pinned at 6.0.3 and Node at 24. The CSP (§9) and `bundle.active` are set, and both
`vite.config.ts` corrections from [AGENTS.md](./AGENTS.md) _Durable failure shapes_ are applied.

_Verified:_ clippy is clean at `-D warnings` over the whole tree, `ts-rs` generation is idempotent
and emits `number` rather than `bigint` for the epoch-ms fields, and `vp run build` produces
`target/release/bundle/{nsis,msi}/` with `vp run verify` confirming `import.meta.env.PROD`.

_Settled by those runs:_ `@typescript/native-preview` is **redundant** — `vp check` reports real
compiler diagnostics for plain `.ts` through tsgolint, so the package is not a dependency of this
repo. The app icons are still the Tauri placeholders; replacing them is a Phase 8 task, alongside
signing.

**Phase 1 — Discovery.** One `ignore` parallel walk over every root at once, the §5.1 prune list,
`.git`-as-file handling, bare/worktree/submodule classification, and `dunce` canonicalisation as
the deduplication key. `discover_roots` collects and sorts; `discover_roots_with` streams, which
is the entry point Phase 3 wires to the channel. Discovery has no fallible signature — an
unreadable root, a permission error, and a broken `.git` are all `ScanSummary.errors` values, so
one stale drive letter cannot cost the user the roots that did scan.

_Verified:_ ten tests in `crates/repo-scan/tests/discover.rs` against a `git`-built fixture tree
cover all four repository kinds, stop-at-first-`.git`, the descend flag, pruning with its count,
overlapping-root and case-differing-root deduplication, a bad root alongside good ones,
`max_depth`, and cancellation. A real run over `C:/Working/Source` found 51 repositories across 566
directories in 58 ms — discovery only, and not the number §2.2 defends, which is Phase 2's. The one
§5.2 case with no test is a genuine permission failure, which has no portable way to stage; the code
path it would take is the same one the unreadable-root test exercises.

_Settled by those runs:_ `gix::discover::is_git` already implements every §5.2 classification
case, so none of it is hand-rolled — see [AGENTS.md](./AGENTS.md). `dirs_pruned` was **0** on the
real tree: because the walk stops at the first `.git`, the prune list only fires on generated
directories sitting _outside_ a repository. It is insurance for oddly-shaped trees, not the main
cost saving, and a future timing regression should not be blamed on it.

**Phase 2 — Tier 0 reads.** `gix` refs, HEAD, upstream resolution, ahead/behind via
`with_hidden` with the cap, stash count, state flags, `catch_unwind` per repo. `read_tier0` for one
repository; `read_tier0_all` / `read_tier0_all_with` fan out over rayon with the §6.4 cancellation
check between repos. The submodule list is **not** here — see §5.1.

_Verified:_ twenty tests in `crates/repo-scan/tests/tier0.rs`, with every ahead/behind topology
agreeing with `git rev-list --left-right --count` and the merge-from-upstream case additionally
pinned by literal, so the `with_boundary` overcount cannot come back. Timings over the real
53-repo tree at `C:/Working`: discovery 107 ms, Tier 0 **146 ms** warm and 479 ms cold, one of the
53 carrying a commit-graph. The commit-graph pair comes from `examples/synth.rs` over a generated
tree of 120 clones, each 100 commits ahead of its upstream over 200 commits of history:
**≈3000 ms without a commit-graph, ≈290 ms with**.

_Settled by those runs:_ **Tier 0 costs about 2.8 ms per repository, not a fraction of one**, and
the residual is `gix::open` re-parsing the global and system config for every repository — gix
0.87.1 exposes no shared snapshot, so that floor stands until it does. The commit-graph is the
single biggest lever on top of it, worth roughly **10×**, which is a good deal more than `gix`'s
own source comment calling it a micro-optimisation suggests. The two numbers look inconsistent
until you notice what the real tree is: almost none of it has a commit-graph, yet it still lands at
2.8 ms/repo, because most repositories are in sync and the equal-tips short-circuit skips the walk
entirely. A tree with genuinely unpushed work pays for the walk, and that is the ≈3000 ms figure.
So the short-circuit, not the commit-graph, is what makes a typical tree fast — and the
commit-graph is what stops an atypical one from being slow.

Both measurements are needed because neither tree alone can give both: no real tree on this machine
reaches 100 repositories, and writing commit-graphs into repositories the user owns is not
something this project does, so scale and the with/without pair are measured on a generated tree
instead. `examples/synth.rs` disables git's own auto-maintenance to keep that baseline honest —
see the trap in [AGENTS.md](./AGENTS.md).

**Phase 3 — Streaming IPC, Tiers 0–1, and a minimal UI.** Canonical state map and tier merge (§6.3),
`subscribe` session channel, `scan_roots` with `ScanId` and `cancel_scan` (§6.4), `pick_root`
over the dialog plugin's Rust API, Pinia mirror store, plain non-virtualized table, progress
indicator. **First point at which the app is useful.**

_Verified:_ 65 Rust tests (10 discovery, 20 Tier 0, 11 Tier 1, 24 in `src-tauri`) and 77 frontend
tests, with `vp check` and `vp run typecheck` clean. The pipeline's smoke test runs against a
**real** `tauri::ipc::Channel` — `Channel::new` needs no `AppHandle`, so the assertions run over
what actually crosses the wire rather than over a mock's idea of it, which is why no `EventSink`
trait exists.

Engine timings over `C:/Working/Source`, 52 repositories across 567 directories, 1 of them carrying
a commit-graph and 17 of them dirty:

| Stage     | Cold    | Warm   | Per repo, warm |
| --------- | ------- | ------ | -------------- |
| Discovery | 54 ms   | 32 ms  | —              |
| Tier 0    | 322 ms  | 136 ms | ~2.6 ms        |
| Tier 1    | 7581 ms | 906 ms | ~17 ms         |

All from `cargo run --release --example scan`. A figure from `vp run dev` is a debug build and means
nothing — it reports ~68 ms per repository for Tier 0 against the ~2.6 ms above.

_Settled by writing it:_ three rules in this document were **wrong as stated** and are now corrected
above. §6.3's "merged field-wise" would have carried a deleted upstream forever — the rule is tier
ownership, and only the genuinely tiered fields survive a merge. §6.2's "keep the id and drop the
others" is not implementable on its own, because Rust starts the pipeline before the invoke's reply
returns; a generation counter is the primary filter and the id is the guard. §3.1's prescribed
`into_index_worktree_iter` reports a staged-only change as clean, and a parked merge with it —
`into_iter` is the one that sees all three kinds of change. Each has a test that fails if the naive
reading is restored. Also settled: a row that is still a `DiscoveredRepo` once Tier 0 has finished
**is** §8.1's total-failure grade and must stop rendering as "counting…", and a bare repository's
worktree fields are `n/a` rather than pending — they can never be computed.

**Tier 1 was pulled forward into this phase.** Leaving the worktree column reading "counting…"
until Phase 4 was a false claim of work in progress, not merely an absent value — it cost a user an
overnight wait, which is what made the distinction concrete. The dirty flag and conflicted count
ship here instead, and the table above is the payoff: the expensive tier is **~24x** the cheap one
cold, so waiting to paint a complete row would hold an answer that is ready in a third of a second
behind one that takes another seven. Eleven tests in `crates/repo-scan/tests/tier1.rs` cover it —
one fixture per trap, plus a `git status --porcelain` oracle over every repository in the tree.

**Phase 4 — Tier 2.** Lazy full status on row expand: the four `git status` column counts and the
submodule list, through `full_status` and `refresh_repo` — which is where `Tier` and
`AppState::has_repo` get their caller, both deferred out of Phase 3 for want of one. The detail
drawer that triggers them, a per-tier breakdown on `ScanEvent::Finished` via `ScanTotals`, and
`ScanEvent::RepoErrors` delivering Tier 0's total failures per batch. Tier 1 belongs to Phase 3 —
see its note above.

_Verified:_ 94 Rust tests (10 discovery, 20 Tier 0, 12 Tier 1, 14 Tier 2, 38 in `src-tauri`) and
111 frontend tests, with `vp check`, `vp run typecheck`, `vp run build` and `vp run verify` clean.
Tier 2's counts are pinned twice
over: a literal per trap, plus a `git status --porcelain` oracle over every repository in the
fixture tree, which is what settled the `IntentToAdd` question rather than a guess. The drawer was
driven in a browser — `counting…` while the read is in flight, then the counts; collapse and
re-expand paints from the mirror with **no second command**; a bare row reads `n/a` and costs no
command at all.

Engine timings over `C:/Working/Source`, 52 repositories, warm:

| Stage  | Warm                  | Note                                                      |
| ------ | --------------------- | --------------------------------------------------------- |
| Tier 0 | ~130 ms               | ~2.6 ms per repo, rayon fan-out                           |
| Tier 1 | ~740–930 ms           | rayon fan-out, early exit on the first status item        |
| Tier 2 | **~35 ms per expand** | sequential, one repository — slowest in that tree ~320 ms |

Tier 2's figure is per repository because that is the only way it ever runs; `--tier2` on
`examples/scan.rs` walks the tree one repository at a time purely to collect the spread. It is
always a **warm** number there, because Tier 1 has just touched the same worktrees — a genuinely
cold single-expand figure would need a separate run and has not been taken. Against Tier 0's
~2.6 ms, one expand is roughly **13x** a whole row's refs, which is the cost the laziness buys back.

_Settled by writing it:_ two of this document's statements were wrong and are corrected above.
§6.1's `full_status -> DetailedStatus` named a type `model.rs` never had, and the merged row is
what the deliverable actually requires. §6.1's blanket "commands accept only a key of the canonical
map" was too strong for `refresh_repo`, whose whole value is retrying a repository that has no row.

**A Phase 3 bug surfaced while taking these timings**, and is fixed here: Tier 1 handed the scan's
shared cancellation flag to `gix`'s `should_interrupt_owned`, which `gix` treats as a flag it may
write — so one repository's early exit aborted whichever neighbours were mid-walk, and could in
principle have ended a whole scan and reported it as cancelled. It presented as an occasional
`Interrupted` error on a random repository. See _Durable failure shapes_ in
[AGENTS.md](./AGENTS.md); `status::private_interrupt` is the fix and `tier1.rs` has a repeat-run
regression test.

**Phase 5 — Filters, sort, grouping, search, persistence.** Filter chips and an inline search over
MiniSearch (§8.3), sortable column headers, group-by-folder, three kinds of persistence — window
geometry, settings, and a row cache — through `persist.rs`, and `open_in` for an editor, a terminal
and the file manager. **The point at which the app stops starting from nothing.**

_Verified:_ 112 Rust tests (10 discovery, 20 Tier 0, 12 Tier 1, 14 Tier 2, 56 in `src-tauri`) and 196
frontend tests across 26 files, with `vp check`, `vp run typecheck`, `vp run build` and `vp run verify`
clean and `vp run types` producing no diff — nothing new is generated, because nothing new crosses
the boundary as a generated type.

The persistence was driven end to end against the real app rather than only through tests, by seeding
the app data directory and reading the log and the files back. A first launch with no cache reports
one and behaves exactly as the app did before there was one. A launch over a seeded cache returns its
rows through `subscribe` **before any scan**, so the window paints immediately; the reconcile then
evicts the one repository that was no longer on disk and rewrites the cache without it. A
hand-written view survives a launch untouched, cached Tier 2 counts are dropped on load, and the exit
write fires on window close. The cache is ~1 KB per repository, so a 500-repo tree is a single
half-megabyte write at the end of a scan.

Engine timings are unchanged, and by construction: `crates/repo-scan/` has no diff in this phase.
Over `C:/Working/Source`, 52 repositories, warm: discovery 36 ms, Tier 0 137 ms, Tier 1 948 ms —
Tier 0 on the nose against Phase 4's table and Tier 1 a hair above the spread it recorded, which is
run-to-run noise on the tier that has 18 dirty worktrees to walk.

_Settled by writing it:_ five things, three of them corrections to this document.

**A launch reconcile must not clear the rows first.** A scan starting over drops them before the
first new one can arrive, which is right for the Scan button and wrong here: the rows on screen are
the ones just restored from the cache, and clearing them would make the window flash empty for the
length of a scan — worse than never having cached them. So `StartScan` takes a `keepRows` option, and
eviction becomes load-bearing rather than hygiene: with no `Reset`, `AppState::retain_scanned` is the
only thing that can remove a repository deleted between sessions.

**The cache has to hold both maps, not just the rows.** `RepoStatus` carries no `git_dir` and every
engine entry point needs the resolved one, so a cache of rows alone would paint a table whose every
row refused to refresh or expand until a scan had rediscovered it — a window that looks ready and is
not.

**A filter is a third place the uncomputed-is-not-zero rule applies, and a sort a fourth.** Excluding
a row whose Tier 1 has not run reports it as one that did not match, so a `dirty` chip applied
mid-scan would quietly call every uncounted row clean. A chip therefore has three answers per row,
not two, and the third is counted and shown. Sorting has the same trap pointed at ordering: `null` is
no answer rather than a small number, so it sorts last in **both** directions.

**§6.1's validation rule needed one more amendment.** `open_in` checks the discovered map, not the
rows: a repository whose HEAD could not be read has no row, and revealing it in the file manager is
exactly how a user finds out why it will not open. That row also gained a drawer of its own for the
same reason — it is the only place its failure is explained and the only place those buttons can be.

**A page's `onMounted` runs before its layout's**, which had been hiding a defect since Phase 3: the
page gave up on the bridge if it was not ready at mount, and it never is, because the layout's
`ping` has not returned yet. Nothing depended on it while roots lived only in memory; with roots
persisted it is the difference between a launch that reconciles and one that shows an empty table.
The page now hydrates on the bridge becoming ready rather than on being mounted.

Also settled, and cheaper than expected: `stale-fetch` needed no new threshold — `fetchStaleness` and
its bands already existed for the fetch-age column, so the chip and the badge cannot disagree. And
`tauri-plugin-window-state` needed no code at all beyond the registration it already had.

**Phase 6 — Watching.** Single debounced watcher over the §7.2 watch set, Rust-side refresh
(§7.5), poll fallback, focus refresh, inotify-limit error handling. **The point at which a row stops
being as old as the last scan.**

_Verified:_ 130 Rust tests (11 discovery, 20 Tier 0, 12 Tier 1, 14 Tier 2, 9 watch, 64 in
`src-tauri`) and 203 frontend tests across 26 files, with `vp check`, `vp run typecheck`,
`vp run build` and `vp run verify` clean. `vp run types` touches two of the generated files —
`DiscoveredRepo` gains `commonDir`, `RepoEvent` gains `watchFailed` — and regenerating a second
time changes nothing, which is the property CI's diff check depends on.

Nine watch tests split in two halves deliberately. The `watch_set` half is pure assertions over the
`git`-built fixture tree: the set per repository kind, `refs/` recursion asserted per entry rather
than only membership, a linked worktree's two directories, and a repository with no commits
contributing no `logs/HEAD`. The `RepoWatcher` half drives a real OS backend and waits for a
callback, because a debounce window has to elapse before anything can arrive — a write under
`plain/.git/refs/` is reported as `plain`, the same write is reported for the linked worktree
watching that directory as its common one, a released repository stops being reported, and a
repository sharing a path with one that just left keeps reporting.

**Driven end to end against the real app**, because none of the above proves the phase works. Over
a seeded three-repository root: the launch reconcile registered 9 paths over 3 repositories; a
`git commit` in one produced exactly **one** push, with the new tip commit landing in the canonical
map; a worktree edit with no `git add` produced **no** event at all and the `git add` that followed
produced one, which is §7.2's rule confirmed rather than assumed; the 20-second poll ran on
schedule; deleting a repository and relaunching evicted its row and released its three watches
(9 paths to 6); and closing the window unwound the poll thread, the watcher and the refresh thread
in that order before the exit cache write. With `enabled: false` the watcher never starts and the
poll alone still picked up a commit made while it was off.

Watch registration timings, from `cargo run --release --example scan -- <path> --watch`:

| Tree                           | Repos | Paths | Register | Per repo |
| ------------------------------ | ----- | ----- | -------- | -------- |
| `C:/Working/Source`            | 52    | 156   | 287 ms   | 5.5 ms   |
| `examples/synth.rs`, 300 repos | 301   | 903   | 1956 ms  | 6.5 ms   |

Exactly **3.00 paths per repository** on both, since neither tree has a linked worktree or an
uncommitted repository. Engine timings are otherwise unchanged: nothing in the tiered path was
touched.

_Settled by writing it:_ four things.

**Registration is slow enough to belong after the terminal event.** At 6.5 ms per repository a
300-repository tree spends two seconds registering, and the first draft synced the watch set before
emitting `ScanEvent::Finished` — which would have made a user wait those two seconds to be told the
scan had finished. It moved to after the event, beside the row cache, for exactly the reason the
cache is there.

**The poll owns Tier 0 and nothing else, which is tier ownership applied to triggers.** A watcher
event is evidence that something changed, so it re-reads Tiers 0 and 1 and invalidates Tier 2. A
poll tick is evidence only that time passed — so nulling `counts` on it would put a `counting…`
flicker into an untouched drawer once a minute, and leaving `dirty` alone while clearing `counts`
would be incoherent anyway. The exception is the watcher's alone, and `refresh_repo` keeps neither:
it was asked for the tiers it named.

**Invalidating Tier 2 obliges the frontend, and Rust cannot discharge it.** `counting…` is a claim
that work is in progress, so a row whose counts were just dropped has to be re-read — but which
rows are expanded is window state that Rust neither knows nor should. `src/scripts/detail.ts`'s
`EnsureDetail` is the discharge, called for every expanded row in a session update, and it is the
existing expand-guard chain lifted out rather than a second copy of it.

**A linked worktree's private directory has its own `refs/`, and watching it is correct.** The
first draft asserted it did not exist and the test failed: per-worktree refs — `refs/bisect/*` and
`refs/worktree/*` — live there rather than in the common directory, so a bisect in that worktree is
visible nowhere else. Both `refs/` trees are watched, which is why the path count is five for a
linked worktree and three for everything else.

Also settled, and cheaper than expected: the reverse index needed to map a watched path to
**several** repositories rather than one — a worktree and its parent share a common directory — and
making it a list gave the reference counting for free, so a worktree going away cannot release the
refs its parent is still watching.

**Phase 7 — Fetch.** Opt-in `git fetch` via CLI with the §8.2 subprocess hygiene, bounded by a
concurrency cap and a repeat guard, with visible last-fetched state and a clear failure surface
for auth problems. **The point at which ahead/behind can be made true rather than only dated.**

_Verified:_ 186 Rust tests (11 discovery, 20 Tier 0, 12 Tier 1, 14 Tier 2, 9 watch, 14 fetch,
26 engine unit, 80 in `src-tauri`) and 240 frontend tests across 29 files, with `vp check`,
`vp run typecheck`, `vp run build` and `vp run verify` clean. `vp run types` adds six generated
files — `FetchStatus`, `FetchOutcome`, `FetchSummary`, `FetchEvent`, `FetchId`, `GitInfo` — and
regenerating a second time changes nothing.

The fourteen integration tests run against a real `git` and a real local origin, each building its
own — a fetch **mutates** the repository it runs in, so the shared `status_tree()` the other tiers
use would have tests moving each other's ahead/behind under parallel execution. Nothing touches
the network: the unreachable-remote case is staged as **loopback port 1**, where a connection is
refused with no DNS lookup, rather than as a `.invalid` hostname that a corporate resolver would
turn into a multi-second hang.

Fetch timings, from `cargo run --release --example scan -- <tree> --fetch --yes` over a generated
tree of **40 local clones** of one bare origin, four concurrent:

| Stage       | Result                                      |
| ----------- | ------------------------------------------- |
| Discovery   | 19 ms over 41 repositories                  |
| Tier 0      | 318 ms                                      |
| Tier 1      | 159 ms                                      |
| **Fetch**   | **3588 ms wall, ~90 ms per repo attempted** |
| Slowest one | 416 ms                                      |

**The origin is local, so that figure is process-spawn cost plus local transport and says nothing
about a network.** A tree of real remotes would be dominated by the slowest one and by the
concurrency cap, which is exactly why the UI counts a repository as done when it has settled
rather than when it started. The bare `origin.git` in that tree came back `NoRemote` with no
process spawned, which is the pre-flight paying for itself outside a test. Engine timings are
otherwise unchanged: the only edit to the tiered path is the `FETCH_HEAD` read below, and it costs
a second `stat` on a linked worktree and nothing at all on every other kind.

_Settled by writing it:_ five things — two of them corrections to text that was simply wrong, one
a gap no section had covered, and two traps worth writing down before someone rediscovers them.

**`FETCH_HEAD` is not always in the common directory, and [AGENTS.md](./AGENTS.md) said it was.**
Measured on git 2.54.0.windows.1 in both directions: a fetch run _in a linked worktree_ writes it
into that worktree's **private** directory and not the common one, while the `refs/remotes/*` it
updates are shared as documented. The old rule is true only of a worktree that has never itself
been fetched and false the moment one is, so `tier0::fetch_head_ms` now takes both directories and
returns the newer. Reading one reports "never fetched" for a repository fetched a second ago — in
the field whose whole job is to say how stale the counts beside it are.

**A fetch triggers the watcher for everything it touches, and no section had joined those two
sentences.** §7.2 puts the git-dir root and `refs/` in the watch set; §8.2 says a fetch writes
`FETCH_HEAD` and remote refs. Left alone a 300-repository pass runs 300 redundant Tier 0+1
refreshes. §7.5 now carries the rule, and the shape of it is the interesting part: the suppression
is a **deadline** so a fetch that panics cannot silently kill a row's live updates, and it
**defers** a notice rather than dropping one so that "only the watcher invalidates Tier 2" stays
true rather than gaining a second exception.

**§8.2 described the post-fetch refresh as a one-field patch**, which is the bug rather than the
fix — corrected above, along with the fetch flags, auto-maintenance, and the fact that
`GIT_TERMINAL_PROMPT=0` closes neither a credential helper's GUI nor `ssh`'s prompts.

**A row resurrected by a fetch completing after eviction is permanent**, and the mechanism causing
it is a documented feature: `merge_tier0` inserts where there was no row so `refresh_repo` can
retry a §8.1 failure. It is in _Durable failure shapes_ rather than here, because it is silent.

**The fetch progress bar is determinate where the scan's is not**, and a reader copying
`ScanProgress.vue` will get that backwards. A scan's denominator does not exist until discovery
ends; a fetch is handed its exact list before the first process starts. The numerator counts
repositories **settled**, never started, or the bar reads 100% with four still running.

Also settled, and cheaper than expected: `resolve_in` moved into the engine so `git` and the
configured editor share one `PATH` walk, and the frontend needed no change to `HandleSessionEvent`
at all — a fetch's rows arrive on the session channel and its existing `EnsureDetail` call already
does the right thing for both an invalidated drawer and an untouched one.

**Phase 8 — Packaging.** Real icons, a WebView2 runtime probe, upgrade-safe installer
configuration, a version-consistency guard, a launch smoke test, and GitHub Actions for both CI and
releases, and an MIT licence. **The point at which someone who will not build it can have it** — v0.1.0
is published, with seven installers across Windows, macOS and Linux.

_Verified:_ 192 Rust tests (11 discovery, 20 Tier 0, 12 Tier 1, 14 Tier 2, 9 watch, 14 fetch, 26
engine unit, 86 in `src-tauri`) and 240 frontend tests across 29 files, with `vp check`,
`vp run typecheck`, `vp run rust`, `vp run versions`, `vp run build`, `vp run verify` and
`vp run smoke` clean, and `vp run types` producing no diff. Six of the `src-tauri` tests are new and
all belong to `webview2.rs`; nothing crossing the wire changed, so nothing new is generated.

**The upgrade path was exercised for real, not reasoned about.** 0.1.0 installed silently, registered
at `HKCU\…\Uninstall\Repo Viewer`; 0.1.1 built and installed over it with `/P`. Afterwards there is
**one** registry entry, not two, its `DisplayVersion` is `0.1.1`, the install location is unchanged,
and `settings.json` in `%APPDATA%` is byte-identical to before. No manual uninstall at any point. The
MSI's `UpgradeCode` is the configured GUID in both the 0.1.0 and 0.1.1 builds, sitting in the Property
table beside `ProductVersion` — the MSI install-and-upgrade cycle itself was **not** run end to end,
because a per-machine MSI needs elevation and the installed binary is blocked anyway (below).

**CI and the release ran, which is the half a working copy cannot prove.** Four jobs green on every
commit — checks plus a build on `windows-latest`, `macos-latest` and `ubuntu-22.04` — and the tag
workflow published v0.1.0 with seven assets: both Windows installer pairs (bootstrapper and
offline), a `.deb`, an `.AppImage`, and a `.dmg`. The macOS artifact is **aarch64**, not universal,
because `macos-latest` is Apple Silicon; an Intel Mac is not covered and nothing has asked for one.

_Settled by writing it:_ four things, two of them corrections to this document.

**§10.4 named the wrong CI host.** It specifies Azure DevOps per house convention; the repository is
on GitHub. Corrected above — ADO would have meant a service connection and a PAT to reach the place
the code already lives.

**The updater was the expensive half of this phase and it is not wanted.** Open decision 5 records
why. What survives from it is the part that always mattered: a place to get the app, and an install
that does not require uninstalling first.

**Signing's premise is now measured, and it is worse than §9 stated.** The enforcement is
ThreatLocker rather than Defender, the split is by path, and it is exact: the binary under
`target/release/` runs while the byte-identical copy the installer places in `%LOCALAPPDATA%` is
denied with `os error 5`. The installer succeeds and the app it installs will not start. Tauri's
NSIS installer also extracts `nsis_tauri_utils.dll` to `%TEMP%` and loads it — that DLL is blocked
separately, so allowlisting the installer is not sufficient on its own.

**A 64-bit process cannot see the WebView2 registration through the obvious registry path**, which
would have made the new probe refuse to start on every machine it was written to protect. Measured
here: the `pv` value exists only under `HKLM\SOFTWARE\WOW6432Node\...`, and the non-redirected path
returns nothing on a machine carrying runtime 152.0.4191.66. See _Durable failure shapes_ in
[AGENTS.md](./AGENTS.md).

Also settled, and more expensive than documented: **the offline installer runs 210–255 MB against
the ~127 MB Tauri's guide states** — around 50x the bootstrapper rather than 30x, and it moves with
the WebView2 runtime version each build downloads. And the two Windows builds
really do collide: running the offline build after the bootstrapper one overwrote both artifacts in
place, which is why the release workflow stages the first pair before the second build starts rather
than after it.

Phases 1–4 are the product. 5–7 make it pleasant. 8 makes it shippable.

---

## 12. Open decisions

Settle each before the phase that depends on it. Numbering is stable — a settled item keeps its
number rather than being removed, because §9 and elsewhere cite these by number.

1. ~~**Write actions.**~~ **Settled:** read-only plus batch fetch, as §1.2 has it. No pull, no
   push, no staging. Fetch is in because ahead/behind is meaningless without it; everything else
   would turn a reporter into a mutator and bring failure modes — a half-applied pull, a rejected
   push — that the row model has nowhere to put. _(Delivered in Phase 7.)_
2. ~~**Nested repos.**~~ **Settled:** stop at the first `.git`, with an opt-in flag to keep
   descending — as §5.1 specifies. With the flag off a submodule is never reached, because the
   parent's `.git` stops the descent above it; with the flag on a submodule gets its own row like
   any other nested checkout. Either way the `submodules` list on the **parent** row is read from
   the parent's config rather than by walking, so the two never disagree — as Tier 2 work, because
   reading it touches the worktree and the index (§5.1). _(Delivered in Phase 1.)_
3. ~~**Fetch policy.**~~ **Settled:** manual only — a per-row button and an explicit batch button.
   Nothing fetches on launch, on a scan, or on a timer. What "all" means is the part that needed
   deciding beyond the original question: it is **what the table is showing**, so filter-to-stale
   then fetch is one gesture and no click ever starts more network operations than the label
   says. A five-minute per-repository guard on batches makes a second press cheap. _(Delivered in
   Phase 7.)_
4. **`gix` pin policy.** The exact-pin half is done — `Cargo.toml` pins `=0.87.1` and AGENTS.md
   treats upgrades as tasks. What is still open is the cadence: who checks for a `gix` minor bump,
   and how often. _(Affects maintenance, not a phase.)_
5. **Signing and update delivery.** Two halves that turned out to be independent, and only one is
   settled. **Delivery: settled.** A **GitHub Release per `v*` tag** — the workflow builds every platform
   and attaches the installers, and a user downloads one and runs it over the version they have.
   There is no in-app updater. `tauri-plugin-updater` was the alternative and it loses on lock-in
   rather than on effort — the endpoint URL and the minisign public key are compiled into every
   build, so the first release fixes both permanently, and an ADO artifact feed cannot be read by an
   unattended app without a token baked into the binary. Manual install costs a user one download
   and one dialog, which is the whole of what the updater was buying.

   **Signing: outstanding.** It does not block delivery — it blocks the delivery being _usable_ on
   a managed machine, which §9 records as measured rather than predicted. The
   route is an Authenticode certificate plus a ThreatLocker publisher rule; where the key lives is
   open while this repository is public and on a personal account. _(Delivered in Phase 8, minus
   signing.)_

6. ~~**`RepoStatus.error` has one slot and two writers.**~~ **Settled:** Tier 0 owns the slot and
   replaces it; Tier 1 **appends** to whatever is already there. The two tiers describe different
   halves of the row and fail independently, so a Tier 1 failure must not erase the reason a Tier 0
   upstream is missing. Appending cannot accumulate across rescans, because Tier 0 replaces the slot
   outright and so restarts the chain on every scan. A per-tier field was the alternative and costs
   a wire change for a case that is rare and only ever displayed. _(Delivered with Tier 1.)_
7. ~~**Where a Tier 2 failure goes.**~~ **Settled:** nowhere on the row. Tier 2 runs on demand and
   can be repeated once per expand, so both of item 6's rules fail here — appending would stack a
   message per expand with only a rescan to clear it, and replacing would erase the reason an
   earlier tier's field is missing. It is also the one tier with a caller waiting on a return
   value, so `full_status` reports the failure as `Err` and the frontend keeps it per path, beside
   the drawer that asked for it. `merge_tier2` therefore does not touch `error` at all, which is the
   only place it differs from `merge_tier1`. _(Delivered in Phase 4.)_
8. ~~**Where a fetch failure goes.**~~ **Settled:** nowhere on the row, and for a sharper reason
   than item 7's. Tier 0 owns `RepoStatus.error` and **replaces** it — and a fetch's own completion
   path re-reads Tier 0 — so a failure written there would be erased by this same operation
   milliseconds later. Appending fails for item 7's reason as well, since a fetch is repeatable per
   row. It goes in a frontend map keyed by path, cleared by that row's next fetch, which makes four
   such maps; the rule that keeps them apart is that **each is owned by one operation and cleared
   by that operation's next attempt**. The four kinds of non-failure — no remote, too soon,
   cancelled, ok — write nothing at all, because reporting "there is nothing to fetch from" as a
   failure would put a red row on a repository that is perfectly fine. _(Delivered in Phase 7.)_

---

## Appendix — the SQL-backed variant

The §1.1 objective. Recorded because it changes nothing structural, which is the point worth
proving: for an offline client app over a local SQL Server, only the engine crate changes.
`crates/repo-scan/` becomes `crates/<domain>/` and its data access uses **`tiberius`**, the pure
Rust TDS client — preserving §10.3's zero-C-dependency property, so no ODBC driver or SQL Native
Client to install alongside the app. `src/`, the IPC layer, and `src-tauri/src/commands/` keep
their shape; commands return query results instead of scan results.

Two things to settle when that project starts: `tiberius`' authentication against a client's SQL
Server (Windows/integrated auth is the usual requirement and the usual friction), and whether
connection pooling is wanted (`bb8` or `deadpool`).

`tauri-plugin-sql` is the wrong tool there — it targets SQLite/MySQL/Postgres, not SQL Server. It
stays relevant only for a future app wanting an embedded local store.

---

## References

- Tauri: [core releases](https://tauri.app/release/core/) · [calling the frontend](https://v2.tauri.app/develop/calling-frontend/) · [capabilities](https://v2.tauri.app/security/capabilities/) · [Windows installer](https://v2.tauri.app/distribute/windows-installer/) · [prerequisites](https://v2.tauri.app/start/prerequisites/) · [Debian](https://v2.tauri.app/distribute/debian/) · [config reference](https://v2.tauri.app/reference/config/) · [Vite guide](https://v2.tauri.app/start/frontend/vite/) (contains the two errors corrected in §3.3) · [project structure](https://v2.tauri.app/start/project-structure/)
- gitoxide: [repo](https://github.com/GitoxideLabs/gitoxide) · [gix docs](https://docs.rs/gix/latest/gix/) · [status module](https://docs.rs/gix/latest/gix/status/index.html) · [status Platform](https://docs.rs/gix/latest/gix/status/struct.Platform.html) · [`is_dirty` source](https://github.com/GitoxideLabs/gitoxide/blob/main/gix/src/status/mod.rs) · [revision walk Platform: `with_hidden` vs `with_boundary`](https://docs.rs/gix/latest/gix/revision/walk/struct.Platform.html) · [feature flags](https://docs.rs/crate/gix/latest/features) · [towards 1.0](https://github.com/GitoxideLabs/gitoxide/issues/470) · [gix-credentials](https://docs.rs/gix-credentials) · [zlib-rs performance](https://trifectatech.org/blog/current-zlib-rs-performance/)
- libgit2 performance: [libgit2#4230](https://github.com/libgit2/libgit2/issues/4230) · [exa#28](https://github.com/ogham/exa/issues/28) · [gitui#2823](https://github.com/gitui-org/gitui/issues/2823) · [git2-rs#347](https://github.com/rust-lang/git2-rs/issues/347)
- Watching and traversal: [notify docs](https://docs.rs/notify/) · [notify-rs](https://github.com/notify-rs/notify) · [ignore::WalkBuilder](https://docs.rs/ignore/latest/ignore/struct.WalkBuilder.html)
- Process-creation cost: [OS primitives benchmark](https://www.bitsnbites.eu/benchmarking-os-primitives/)
- WebView2 availability: [Evergreen vs fixed](https://learn.microsoft.com/microsoft-edge/webview2/concepts/evergreen-vs-fixed-version) · [distribution](https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution) · [delivery to Windows 10](https://blogs.windows.com/msedgedev/2022/12/14/delivering-microsoft-edge-webview2-runtime-to-managed-windows-10-devices/)
- Linux floor: [Ubuntu archive: `libwebkit2gtk-4.1-0`](https://packages.ubuntu.com/search?keywords=libwebkit2gtk-4.1-0&searchon=names&suite=all&section=all) · [tauri#12463](https://github.com/tauri-apps/tauri/issues/12463)
- Toolchain: [Vite 8 / Rolldown](https://vite.dev/blog/announcing-vite8-beta) · [what changed](https://certificates.dev/blog/rolldown-and-vite-8-what-changed) · [Vite `build.minify`](https://vite.dev/config/build-options) · [TypeScript 7.0 RC announcement](https://devblogs.microsoft.com/typescript/announcing-typescript-7-0-rc/) · [vuejs/language-tools#5381 (TS 7)](https://github.com/vuejs/language-tools/issues/5381) · [vue-tsgo](https://github.com/NikhilVerma/vue-tsgo) · [Node.js release schedule](https://endoflife.date/nodejs) · [Corepack is not distributed with Node 25+](https://socket.dev/blog/node-js-tsc-votes-to-stop-distributing-corepack) · [cargo#8519](https://github.com/rust-lang/cargo/issues/8519) · [Rust: automatic verbatim paths on Windows](https://github.com/rust-lang/rust/pull/89174) · [`dunce`](https://docs.rs/dunce) · [Tauri JS mocks](https://v2.tauri.app/develop/tests/mocking/)
- Prior art: [gitpane](https://github.com/affromero/gitpane) · [RepoZ](https://github.com/awaescher/RepoZ) · [mu-repo](https://github.com/fabioz/mu-repo) · [gr](https://github.com/mixu/gr)
- Sibling in-house repos, not public: a dashboard app (frontend stack and conventions) · a docs site (`src/composables/useSearch.ts`, the MiniSearch pattern)
