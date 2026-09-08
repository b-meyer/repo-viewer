# repo-viewer

A cross-platform desktop app: point it at a folder and get a live dashboard of every Git repo
beneath it — branch, ahead/behind, dirty state, file counts — without opening each one in an IDE.

Status: **Phase 1 complete.** The workspace, the toolchain, and the app shell build and run end to
end: `vp check`, `vp run typecheck`, `vp test run`, and `vp run rust` are green, `vp run types`
generates the bindings, and `vp run build` produces both Windows installers. Discovery finds and
classifies every repository under a root; nothing reads Git objects yet. Next step is Phase 2,
Tier 0 reads.

---

## 1. What this is

### 1.1 Two goals

1. **The tool.** A multi-repo Git dashboard for personal use, shared with colleagues at CIT who
   want it.
2. **A delivery pattern.** Establish whether Vue/web skill transfers to shipping a desktop app to
   a smaller client — a site that cannot run a web server, or has connectivity too poor to rely
   on, but needs an intranet-style app against a **local SQL database**.

Goal 2 drives most of what follows. It is why the frontend stack matches WPT.Dashboard rather
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
  - `git` CLI subprocess for fetch/pull/push only
  - path normalization, editor/terminal/file-manager launch
  - plugin calls (dialog, opener, store) — the frontend never imports a plugin package
  - settings and cache persistence
```

The boundary is structural, not aspirational: `src/` cannot import the engine, because one is
TypeScript and the other a Rust crate.

### 2.2 The scan is tiered

Most of what the dashboard shows costs almost nothing; only file counts are expensive. Three
tiers stream independently, so a row appears before any worktree is touched.

| Tier                | Cost per repo                                                                                                                                  | Yields                                                                                               | When                                                                     |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| **0 — refs only**   | sub-millisecond; reads `.git/HEAD`, `packed-refs`, loose refs, revwalk with commit-graph                                                       | branch, ahead/behind, upstream, stash count, state flags (rebase/merge/bisect/detached), last commit | immediately, every scan                                                  |
| **1 — dirty flag**  | early-exit: first item from the status iterator, **untracked files included**; conflicted count read from index stage entries, no worktree I/O | clean/dirty boolean, conflicted count                                                                | streams in right after Tier 0                                            |
| **2 — full counts** | full index↔worktree diff                                                                                                                       | staged / unstaged / untracked / conflicted                                                           | lazily: expanded rows, explicit refresh — never in the default scan path |

Tier 0 alone answers "which repos have unpushed commits?" with zero worktree I/O. This tiering
matters more to perceived performance than any library choice.

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
| `tracing` / `tracing-subscriber` | 0.1.44 / 0.3.23   | scan timings                                                               |
| `ts-rs`                          | 12.0.1            | TypeScript type generation (§4.2)                                          |
| `dunce`                          | 1.0.5             | strips the `\\?\` prefix `canonicalize()` returns on Windows (§5.2)        |
| `tempfile`                       | 3.27.0            | dev-only, test fixtures                                                    |
| `tauri-plugin-dialog`            | 2.7.3             | native folder picker                                                       |
| `tauri-plugin-store`             | 2.4.4             | JSON cache                                                                 |
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
  clean. Tier 1 uses `repo.status(Discard)?.untracked_files(Collapsed).into_index_worktree_iter(..)`
  with `should_interrupt`, and takes the first item. Clean repos pay a full ignore-aware worktree
  walk; Phase 4 records that cost.
- **`with_boundary` is not `^upstream`.** Its docs say a boundary "is distinctly different from
  exclusive revspecs" — it stops the walk at the given ids but does not hide their ancestors, so
  any merged history overcounts. Ahead/behind is `rev_walk([local]).with_hidden([upstream])`
  counted, then swapped. `with_hidden` warns that disjoint histories may traverse everything, so
  the walk is capped (~1000, shown as "1000+"). Cost depends on a commit-graph being present;
  without one every commit is an object decode. Isolated in one module — that module is the seam
  if the primitive changes; there is no backend trait.
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

The WPT.Dashboard stack at latest versions. Greenfield has no migration cost, so this repo runs
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
validator: plugins are reached through Rust commands (§3.1), and everything the frontend receives
is typed end to end by `ts-rs` (§4.2).

**Every version exact, declared once** in the root `pnpm-workspace.yaml` `catalog:` block.
Catalogs are a pnpm-workspace feature, so that file exists even though this is a single package.
Bump with `vp update -L <pkg>`, never by widening a range.

**Virtualization is deferred.** 500 rows of simple DOM does not need it, and it costs real
complexity in measurement, sticky headers, keyboard nav, and find-in-page. Keep row markup
fixed-height, measure, and add `@tanstack/vue-virtual` 3.13.36 past a measured threshold
(~1000 rows, or a p95 frame-budget miss).

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
2. **`target/` is at the workspace root.** The template's `src-tauri/.gitignore` entry for
   `/target/` stops matching; add `/target/` at the repo root.
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

The tree it builds now covers all four repository kinds plus a pruned directory and a nested
checkout. Two further fixtures pin the §3.1 traps and arrive with the phases that need them: a
repo whose only change is one untracked file (Phase 4 — Tier 1 must say dirty), and two
ahead/behind topologies, a merge from upstream into local and a criss-cross merge (Phase 2),
asserted against `git rev-list --left-right --count local...upstream`.

Frontend code that reaches `ipc.ts` is tested with `mockIPC` from `@tauri-apps/api/mocks`,
which intercepts `invoke` and `Channel` traffic without a webview; no hand-rolled mocks.

**No `criterion`.** Its repeated-sampling model is the wrong shape for a whole-tree scan — 100
samples of a multi-second operation is a five-minute run fought with `sample_size`. The Phase 2
deliverable is one recorded number, which is `std::time::Instant` in the example below. Reach for
`criterion` later for micro-level pieces (`ahead_behind` on one repo, the prune predicate) if
they profile hot.

### 4.4 Running the engine without the GUI

`crates/repo-scan/examples/scan.rs`, run as
`cargo run --release --example scan -- C:/Working/Source`. Cargo compiles `examples/` during
`cargo test`, so it cannot rot, and it needs no extra crate, manifest, or `clap`. That covers
producing the timing number, debugging one repo without a webview in the way, and checking
behaviour in a CI container. Promote to `crates/repo-scan-cli/` with `clap` only once it wants
subcommands and flags.

### 4.5 How this maps onto WPT.Dashboard

| WPT.Dashboard                                               | repo-viewer                                     |
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
- The `submodules` list on a parent row comes from the parent's config in Tier 0, never from the
  walk. Whether a submodule also gets a row of its own is exactly what the descend flag decides.

An unspecified `threads` means **half** the core count, not all of it: from Tier 0 onwards the
walk and the rayon pool run at the same time, so taking both defaults would put twice as many
threads on the machine as it has cores.

Discovery yields `DiscoveredRepo` — path, name, parent, kind, and the resolved Git directory —
rather than a partial `RepoStatus`. The §8.1 Tier 0 fields are not `Option`, because a row that
has been read always has them, and discovery has read nothing; a "partial" row would have to lie.
This is what `RepoFound` carries, and Tier 0 turns it into a `RepoStatus`. `ScanOpts` and
`ScanSummary` cross the same boundary and live in `model.rs` beside it.

The Git directory is resolved once, here: `<path>/.git` for a normal repo, `<path>` itself when
bare, and the private directory the `.git` _file_ names for a worktree or submodule. Later phases
consume it rather than re-resolving — Tier 0 opens it, and §7.2's watch set is registered against
it.

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
subscribe(on_event: Channel<RepoEvent>)           // once at startup; lives for the session
scan_roots(roots: Vec<PathBuf>, opts: ScanOpts, on_event: Channel<ScanEvent>) -> ScanId
cancel_scan(id: ScanId)
refresh_repo(path: PathBuf, tier: Tier) -> RepoStatus
full_status(path: PathBuf) -> DetailedStatus      // Tier 2, on demand
fetch_repos(paths: Vec<PathBuf>, on_event: Channel<FetchEvent>)   // git CLI
open_in(path: PathBuf, target: OpenTarget)        // editor | terminal | file manager
pick_root() -> Option<PathBuf>                    // native folder dialog, via the plugin's Rust API
add_root(path) / remove_root(path) / list_roots()
```

Commands registered via `invoke_handler` are callable by all windows by default and need no
capability declaration. Because the plugins are reached only from Rust (§3.1), no plugin
permission is declared either: `src-tauri/capabilities/default.json` grants `core:default` and
nothing else. Filesystem work inside our own commands is not constrained by any plugin scope —
the Rust side is trusted — which is why all fs access stays in Rust and `tauri-plugin-fs` is not
used. The corollary is that commands validate their own inputs: `open_in`, `refresh_repo` and
`full_status` accept only a path that is already a key in the canonical repo map (§6.3), never
an arbitrary string from the webview.

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

Every `ScanEvent` carries its `ScanId`. The frontend keeps the id of the scan it asked for and
drops batches from any other, so a rescan or a root change mid-scan cannot interleave stale rows
with fresh ones.

### 6.3 Rust owns the canonical state

A tiered stream has a merge problem: a Tier 0 result for a repo arriving after its Tier 1 result
carries `dirty: None`, and a naive "replace the row" would erase a value the UI already shows.
The merge therefore happens once, in Rust. `src-tauri/src/state.rs` holds
`HashMap<PathBuf, RepoStatus>`; each tier result is merged field-wise into the existing row, and
the **full merged row** is what goes over the channel. The Pinia store is a mirror keyed by path
— it never merges, never infers, and never holds a value Rust does not. The same map is what the
JSON cache serialises, so there is exactly one source of truth on each side of the IPC boundary.

### 6.4 Cancellation

Every scan gets an `Arc<AtomicBool>`. Discovery checks it and returns `WalkState::Quit`; `gix`
status calls take it via `should_interrupt`; the rayon fan-out checks it between repos.
`cancel_scan` flips it, a root change flips the previous scan's flag before starting the next,
and window close flips all of them. The scan body runs on `tauri::async_runtime::spawn_blocking`
so the Tauri runtime's threads are never occupied by a walk.

---

## 7. Live updates

### 7.1 One watcher, many paths

`notify` spawns a thread per `Watcher` object, so 300 watchers means 300 threads. Create **one**
`recommended_watcher()` and call `watch()` once per repo.

### 7.2 What to watch

Three watches per repo, all inside the git dir:

- the git dir root, **non-recursive** — catches `HEAD`, `index`, `packed-refs`, `FETCH_HEAD`,
  `ORIG_HEAD`, `MERGE_HEAD`, and the `index.lock` bursts (§7.3)
- `refs/`, **recursive** — a non-recursive watch sees only direct children, so it misses every
  slash-named branch (`refs/heads/feature/x`) and every remote-tracking update
  (`refs/remotes/origin/main`), which is exactly the ahead/behind signal. The refs tree is a few
  dozen small files, so recursion here is free.
- `logs/HEAD` — appended on every commit, checkout, and reset regardless of branch name

A linked worktree has a private git dir (`.git/worktrees/<name>/`, its own `HEAD` and `index`)
and a shared common dir (refs, objects); both are watched, resolved through `gitdir:` and
`commondir`.

The worktree itself is never watched: recursively watching worktrees means watching
`node_modules`, which is how tools burn CPU and blow past inotify limits. Worktree edits are
picked up by the poll, by refresh-on-focus, or by the `index` change that follows any `git add`.

### 7.3 Debouncing is mandatory

Git does not write `.git/index` once; it writes `index.lock`, writes, then renames, so a single
`git add` produces a create/modify/remove burst. `notify-debouncer-full` with a ~300–500 ms
window plus a per-repo refresh cooldown is required to avoid a refresh storm. Never let a watcher
callback block: if it stalls, OS events pile up in the kernel buffer and are silently dropped on
overflow.

### 7.4 Watching is an optimization, never the source of truth

- **Linux:** inotify has a per-user watch limit; exceeding it surfaces as "No space left on
  device". Detect that specific error and surface actionable guidance
  (`sysctl fs.inotify.max_user_watches=524288`) rather than failing opaquely.
- **macOS:** FSEvents cannot observe files the process does not own; Docker on Apple Silicon
  returns `os error 38`.
- **All platforms:** `notify`'s own docs warn it "may fail to receive all events" at high file
  counts, and that backends are "not a 100% reliable source".

So always ship a low-frequency poll (Tier 0 every ~60 s, configurable) and a refresh-on-focus,
with `PollWatcher` available as a manual fallback for network mounts and containers.

### 7.5 Refresh happens in Rust

A debounced event never crosses the IPC boundary as a "something changed" notice. The watcher
task re-runs Tier 0 and Tier 1 for that repo, merges the result into the canonical map (§6.3),
and pushes the merged row on the session channel. The poll and the focus handler
(`WindowEvent::Focused(true)`) take the same path. The frontend has nothing to do but render
what arrives.

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
    upstream: Option<String>,
    ahead: Option<u32>,          // None = no upstream configured; capped, see §3.1
    behind: Option<u32>,
    last_commit: Option<CommitSummary>,
    stash_count: u32,
    state: RepoState,            // Clean | Merging | Rebasing | Bisecting | CherryPicking
    last_fetched_ms: Option<u64>,  // mtime of FETCH_HEAD; None if never fetched (§8.2)

    // Tier 1
    dirty: Option<bool>,         // None = not yet computed; true includes untracked files
    conflicted: Option<u32>,     // index entries with stage > 0; None = not yet computed

    // Tier 2
    counts: Option<FileCounts>,  // staged, unstaged, untracked, conflicted
    submodules: Vec<SubmoduleStatus>,

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

Persisted to the JSON store so launch paints last-known state immediately, then reconciles. Every
cached row renders with its `scanned_at` age until refreshed.

### 8.2 Ahead/behind is relative to the last fetch

Ahead/behind is measured against the local remote-tracking ref (`refs/remotes/origin/*`), which
is only as fresh as the last `git fetch`. An app whose selling point is "what haven't I pushed?"
is misleading if it shows stale numbers silently. So:

- Show a per-repo **"last fetched"** timestamp and visually degrade rows past a threshold.
- Fetching is explicit, opt-in, and rate-limited — never part of a scan, and never on launch.
  Fetching 300 repos unprompted is hostile.
- **Fetch via the `git` CLI.** Fetch is where credential helpers, SSH config (`~/.ssh/config`,
  agents, jump hosts), corporate proxies, and custom transports matter, and where getting it
  wrong means hanging on a credential prompt with no UI. This is also why `keyring` is not
  needed: delegate to the credential helper already installed (Git Credential Manager on
  Windows, Keychain on macOS).
- **Subprocess hygiene.** `GIT_TERMINAL_PROMPT=0` so a missing credential fails instead of
  waiting on a terminal that does not exist; `CREATE_NO_WINDOW` (`creation_flags(0x0800_0000)`)
  on Windows or every fetch flashes a console; a per-process timeout (~60 s) because SSH and
  proxy stalls are otherwise indefinite; at most ~4 concurrent fetches. `last_fetched_ms` is
  re-read from `FETCH_HEAD` after each completion and pushed on the session channel.

### 8.3 Search

`minisearch` 7.2.0. CIT Quickwire's `qdocs` runs it in production
(`src/composables/useSearch.ts`); the tuning below is borrowed from there. At a few hundred
short strings a `String.includes` filter in a `computed` would also do; MiniSearch is here for
parity with the qdocs pattern, not because the corpus needs it.

1. **`shallowRef`, never `ref`, for the instance.** The index is a large nested structure; deep
   reactivity over it is a performance disaster.
2. **Module-scoped singleton.** Instance and UI state (`open`, `query`, `selectedIndex`) at
   module scope; the composable returns handles. One index, shared.
3. **`fields` vs `storeFields` are different lists.** `fields` is searched; `storeFields` is what
   comes back — set it and results are flat (`r.slug`), unset and you dig through `r.obj.*`.
4. **Length-conditional fuzziness:** `fuzzy: (term) => term.length >= 5 ? 0.2 : false`, plus
   `prefix: true`. Exact-only under 5 characters matters more here than in docs, since repo names
   are full of short fragments (`api`, `db`, `ui`, `ssg`).
5. **AND first, OR as fallback.** Search `combineWith: 'AND'`; if empty, re-run with `'OR'`.

Also carry `boost` for field weighting (repo name over its path), `MIN_QUERY_LENGTH = 2`, and
`results` as a `computed` over `query`.

**The corpus is live, not static.** qdocs fetches a prebuilt index once; here the corpus _is_ the
repo set, streaming in tier by tier and mutating on watcher events. So build from the Pinia store
with no `fetch` and no build-time artifact; `add` on `RepoFound` batches and **`replace(doc)`**
when a row changes. MiniSearch 7 has the full incremental surface — `add`, `addAll`,
`addAllAsync`, `remove`, `removeAll`, `replace`, `discard`, `discardAll`, `vacuum`, `has`,
`getStoredFields`, `search`, `autoSuggest`, plus `toJSON` / static `loadJSON`. Prefer `discard`
over `remove` and let auto-vacuum reclaim; do not rebuild on every change.

Index only stable, cheap fields — repo name, path, branch, upstream. Never Tier 2 counts: they
are lazy and mostly unknown, so indexing them means reindexing on every tier completion for no
search value.

Do not add an `optimizeDeps.include` entry for it. qdocs needs one because `minisearch` is
reached _through_ a library excluded from pre-bundling; here it is a direct dependency imported
from `src/`, so Vite's initial scan pre-bundles it with no configuration.

---

## 9. Packaging

Targets: NSIS `.exe` and WiX `.msi` on Windows (MSI is Windows-build-only); `.dmg`/`.app` on
macOS; `.deb` and `.AppImage` on Linux. ARM64 Windows needs
`rustup target add aarch64-pc-windows-msvc` plus the VS "C++ ARM64 build tools" component; macOS
universal builds use `universal-apple-darwin`.

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
    "webviewInstallMode": { "type": "downloadBootstrapper", "silent": true }
    // switch to "offlineInstaller" for air-gapped or egress-blocked fleets
  },
  "linux": { "deb": { "depends": ["libwebkit2gtk-4.1-0", "libgtk-3-0"] } }
}
```

No system tray is planned, which drops `libappindicator3-1` — one fewer Linux dependency. Keep it
that way unless a tray is wanted.

**Signing is a Phase 8 prerequisite for goal 1, not a nicety.** CIT-managed Windows machines run
Defender for Endpoint with tamper protection; it denies _execution_ of freshly downloaded,
low-prevalence executables outright (`os error 5` with correct ACLs), independent of SmartScreen.
An unsigned installer handed to a colleague does not get a click-through, it gets blocked. The
two workable paths are an Authenticode certificate trusted by the tenant, or an IT-issued allow
indicator by publisher or hash — either is an IT request with lead time, so it is filed before
Phase 8 starts. macOS Gatekeeper and notarization matter only if a Mac build ships. How
colleagues receive updates is open decision 5.

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
produces a native message box naming the fix rather than a blank exit. Keep
`downloadBootstrapper` for general distribution and build a second `offlineInstaller` artifact
(~127 MB) for locked-down environments.

**Linux is version-gated, not just dependency-managed.** A Tauri v2 `.deb` declares
`libwebkit2gtk-4.1-0` and `libgtk-3-0`, so `apt` pulls them — but 4.1 exists in jammy 22.04,
noble 24.04, 25.10 and 26.04, and **not** in focal 20.04. Tauri v2 requires 4.1 specifically.
Declare Ubuntu 22.04 / Debian 12 as the floor and build Linux artifacts in a 22.04 container:
glibc compatibility is forward-only, so building on a newer system raises the minimum glibc and
produces binaries that fail on the stated floor.

### 10.2 The Git CLI is a real but graceful dependency

Viewing status needs no Git — that is in-process `gix`. Fetch/pull/push do (§8.2). Detect its
absence at startup and disable those actions with an explanation rather than failing at click
time; the rest of the app stays fully functional.

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

**Azure DevOps** (`azure-pipelines.yml`), per house convention.

| Pool / container         | Builds                               | Notes                                                  |
| ------------------------ | ------------------------------------ | ------------------------------------------------------ |
| `windows-latest`         | NSIS `.exe`, WiX `.msi`, x64 + arm64 | MSI cannot be cross-built; the leg that matters for v1 |
| `macos-latest`           | `.dmg`, `universal-apple-darwin`     | set `minimumSystemVersion: 10.15`                      |
| `ubuntu-22.04` container | `.deb`, `.AppImage`                  | **must be 22.04, not `ubuntu-latest`** — glibc floor   |

Given the audience — a single developer plus colleagues at CIT — treat the Windows leg as the pipeline
and the other two as opt-in proof of portability. Do not build a three-platform release before
the Windows one is used in anger.

CI invokes the toolchain as `pnpm exec vp …` because `vp` is not global on an ADO agent — a
pipeline detail, not a local pattern. Agents use Node 24 (the Active LTS line) via
`NodeTool@0` reading `.node-version`, and install pnpm with `npm i -g pnpm@<pinned>`; pnpm then
enforces the `packageManager` field itself. Rust legs run `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, and `cargo test` alongside the frontend's
`vp check`.

Add a per-platform smoke test that launches the binary headless and asserts the webview
initializes, so a missing-runtime regression is caught in CI rather than by a user. Also assert
the built bundle's `import.meta.env.PROD` flag (§3.3).

---

## 11. Roadmap

Each phase gets a runbook in `docs/` when it starts, written against the tree as it exists then,
and is deleted when the phase completes — durable facts move into README.md and AGENTS.md.
No phase is currently open; Phase 2 gets the next one.

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

_Verified:_ nine tests in `crates/repo-scan/tests/discover.rs` against a `git`-built fixture tree
cover all four repository kinds, stop-at-first-`.git`, the descend flag, pruning with its count,
overlapping-root and case-differing-root deduplication, a bad root alongside good ones, and
`max_depth`. A real run over `C:/Working/Source` found 51 repositories across 566 directories in
58 ms — discovery only, and not the number §2.2 defends, which is Phase 2's. The one §5.2 case with
no test is a genuine permission failure, which has no portable way to stage; the code path it would
take is the same one the unreadable-root test exercises.

_Settled by those runs:_ `gix::discover::is_git` already implements every §5.2 classification
case, so none of it is hand-rolled — see [AGENTS.md](./AGENTS.md). `dirs_pruned` was **0** on the
real tree: because the walk stops at the first `.git`, the prune list only fires on generated
directories sitting _outside_ a repository. It is insurance for oddly-shaped trees, not the main
cost saving, and a future timing regression should not be blamed on it.

**Phase 2 — Tier 0 reads.** `gix` refs, HEAD, upstream resolution, ahead/behind via
`with_hidden` with the cap, stash count, state flags, `catch_unwind` per repo. _Deliverables:_
the topology fixtures of §4.3 passing against `git rev-list`, and a recorded timing over a real
tree of 100+ repos via `examples/scan.rs`, taken twice — with and without
`.git/objects/info/commit-graph` present. This is the number the whole design defends.

**Phase 3 — Streaming IPC and minimal UI.** Canonical state map and tier merge (§6.3),
`subscribe` session channel, `scan_roots` with `ScanId` and `cancel_scan` (§6.4), `pick_root`
over the dialog plugin's Rust API, Pinia mirror store, plain non-virtualized table, progress
indicator. **First point at which the app is useful.**

**Phase 4 — Tiers 1 and 2.** Dirty flag from the status iterator with untracked files included,
conflicted count from the index; lazy full status on row expand. _Deliverable:_ Tier 1 timing
over the same tree as Phase 2, and a check that partial-state rendering is honest (§8.1).

**Phase 5 — Filters, sort, grouping, search, persistence.** Filter chips, MiniSearch (§8.3), JSON
cache and settings via the store plugin from Rust, window state, `open_in` with the repo-map
check.

**Phase 6 — Watching.** Single debounced watcher over the §7.2 watch set, Rust-side refresh
(§7.5), poll fallback, focus refresh, inotify-limit error handling.

**Phase 7 — Fetch.** Opt-in `git fetch` via CLI with the §8.2 subprocess hygiene, rate-limited,
with visible last-fetched state and a clear failure surface for auth problems.

**Phase 8 — Packaging.** Signing or an allow indicator secured first (§9), then the Windows
installer, then the CI matrix.

Phases 1–4 are the product. 5–7 make it pleasant. 8 makes it shippable.

---

## 12. Open decisions

Settle each before the phase that depends on it. Numbering is stable — a settled item keeps its
number rather than being removed, because §9 and elsewhere cite these by number.

1. **Write actions.** Read-only plus batch fetch is the §1.2 scope. Confirm it stays that way, or
   accept a much larger surface. _(Blocks Phase 7.)_
2. ~~**Nested repos.**~~ **Settled:** stop at the first `.git`, with an opt-in flag to keep
   descending — as §5.1 specifies. With the flag off a submodule is never reached, because the
   parent's `.git` stops the descent above it; with the flag on a submodule gets its own row like
   any other nested checkout. Either way the `submodules` list on the **parent** row is read from
   the parent's config in Tier 0 rather than by walking, so the two never disagree.
   _(Delivered in Phase 1.)_
3. **Fetch policy.** Manual-only, or opt-in periodic background fetch? Recommend manual plus an
   explicit "fetch all"; auto-fetch over VPN on 300 repos is a support burden. _(Blocks Phase 7.)_
4. **`gix` pin policy.** The exact-pin half is done — `Cargo.toml` pins `=0.87.1` and AGENTS.md
   treats upgrades as tasks. What is still open is the cadence: who checks for a `gix` minor bump,
   and how often. _(Affects maintenance, not a phase.)_
5. **Signing and update delivery.** Which of the two §9 paths — Authenticode certificate or an
   IT allow indicator — and how colleagues get new versions: a share path with a version check
   in-app, or `tauri-plugin-updater` against an ADO artifact feed. Recommend the certificate plus
   the updater; the allow-indicator route has to be repeated per build hash. _(Blocks Phase 8.)_

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
- Sibling repos: `WPT.Dashboard` (frontend stack and conventions) · `CIT Quickwire/qdocs` (`src/composables/useSearch.ts`, the MiniSearch pattern)
