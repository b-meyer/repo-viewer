# Phase 0 — Environment and scaffold

Runbook for the first executable phase of [PLAN.md §11](../PLAN.md). It is written so a session
with no prior context can run it top to bottom. It is deleted when Phase 0 is done: everything
durable in it ends up in [README.md](../README.md) (build requirements) and
[AGENTS.md](../AGENTS.md) (rules and traps), and the scaffolded files speak for themselves.

Read [AGENTS.md](../AGENTS.md) first. Two of its rules bind every step here: **Git is read-only
for agents** (prepare commands, do not commit), and **every dependency version is exact**.

---

## 0. Gate: the Rust toolchain must execute

Defender for Endpoint on the CIT-managed machine denies execution of the rustup-installed
binaries. `rustup` runs; `cargo` and `rustc` fail with `Access is denied (os error 5)`. Nothing
in the rest of this runbook works until that is lifted, and no amount of local configuration
lifts it.

**Check** (from any directory):

```sh
cargo --version && rustc --version
```

Both print a version → continue to §1. Either prints `Access is denied` → stop and file the IT
request below; nothing else in this phase can proceed.

**IT request.** Allow execution under these paths for this user. The third is precautionary:
`cargo build` and `cargo test` compile and *run* fresh executables (build scripts, test
binaries, the app itself) under the repo's `target/`, and a rule that blocks freshly created
binaries will hit those next.

```
%USERPROFILE%\.cargo\**
%USERPROFILE%\.rustup\**
C:\Working\Source\**
```

Use the same channel that approves `winget` packages. Once granted, **verify with a real
build, not a version print** — a toolchain can be installed, on disk, and unusable:

```sh
cd "$TEMP" && cargo new --bin linkcheck -q && cd linkcheck && cargo build && ./target/debug/linkcheck.exe
```

Expect `Hello, world!`. This also proves the MSVC linker is reachable.

## 1. Inventory of what is already on the machine

Verified on the primary dev machine; re-check rather than assume.

| Tool | State | Action |
|---|---|---|
| `rustup` 1.29.1, `stable-x86_64-pc-windows-msvc` | installed, blocked (§0) | IT request |
| MSVC 14.51 (VS 18 Enterprise), Windows 11 SDK 10.0.26100 | present | none |
| WebView2 Runtime | present (`pv` = 152.x) | none |
| Node 22.21.1 via nvm-windows (`C:\Program Files\nvm`) | wrong major | install 24 (§2) |
| pnpm 10.33.1 | present | pinned by `packageManager` (§4) |
| global `vp` 0.2.2 (`~/.vite-plus/bin/vp`) | present | fine — defers to the project's local `vite-plus` |
| `create-tauri-app` 4.6.2, `@tauri-apps/cli` 2.11.4, `oxlint` 1.79.0, `tsgo` 7.0.0-dev.20260707.2 | all execute via `pnpm dlx` | none — the Defender block is specific to the rustup binaries |

## 2. Node 24

`vite-plus` 0.3.0 requires `^20.19.0 || ^22.18.0 || >=24.11.0`; the project standard is Node 24,
the Active LTS line (26 becomes LTS on 2026-10-28). Current 24.x is **24.20.0**.

```sh
nvm install 24.20.0 && nvm use 24.20.0 && node --version
```

Corepack is not used anywhere: pnpm enforces its own version from `packageManager` (§4).

## 3. Scaffold into a sibling directory, then move

`create-tauri-app` writes its own `README.md` and `.gitignore`; scaffolding in place would
clobber ours. Scaffold beside the repo and move selectively.

```sh
cd C:/Working/Source/b-meyer
pnpm create tauri-app@4.6.2 repo-viewer-scaffold \
  --template vue-ts --manager pnpm --tauri-version 2 \
  --identifier net.citsolutions.repoviewer --yes
```

Then, from `repo-viewer/`:

```sh
S=../repo-viewer-scaffold
mv $S/src-tauri $S/src $S/index.html $S/vite.config.ts $S/package.json \
   $S/tsconfig.json $S/tsconfig.node.json $S/.vscode .
cat $S/.gitignore            # fold its entries into ours in §6; do not move it
rm -rf $S
```

Do not move `README.md`. Anything the template puts under `src/` (`App.vue`, `assets/`,
`components/Greet.vue`, `main.ts`, `style.css`) is placeholder and gets replaced in §5.

## 4. Toolchain: `vite-plus`, pnpm, catalog

1. `vp migrate` in the repo root. It rewrites `vite` → `vite-plus` imports, moves scripts to `vp`
   commands, and installs. Expect "further manual adjustments" — that is the rest of this
   section.
2. `package.json`:
   - `"packageManager": "pnpm@10.34.5"` — pnpm 10 switches itself to this version on first
     run; `vp` reads the same field to pick the package manager.
   - `"engines": { "node": ">=24.11.0 <25" }` and a root `.node-version` containing `24.20.0`.
   - `"private": true`, `"type": "module"`.
   - Every dependency entry is `"catalog:"`; no version literals in `package.json`.
3. `pnpm-workspace.yaml` — one `catalog:` block, every version exact, taken from
   [PLAN.md §3.2](../PLAN.md). Catalogs are a workspace feature; the file exists even though
   there is one package. Do **not** add `oxlint`, `oxfmt`, `oxlint-tsgolint`, or `tsdown`
   (they arrive inside `vite-plus`), and do **not** add any `@tauri-apps/plugin-*` package or
   `zod` (plugins are called from Rust — [PLAN.md §3.1](../PLAN.md)).
4. `vp install`. Never `pnpm install` directly from here on.
5. **TypeScript is 6.0.3.** `vue-tsc`'s peer range admits 7.x and 7.0.2 is `latest`; installing
   7 breaks `.vue` type-checking. `@typescript/native-preview` supplies the separate `tsgo`
   binary for plain `.ts`. Phase 0 includes one check on this pair (§8, item 6).

## 5. Frontend skeleton

Lay out `src/` as in the [README layout](../README.md#layout). For Phase 0 only the shell needs
to exist: `layout/App.vue`, `pages/index.vue` with a heading, `scripts/ipc.ts` exporting one
typed `ping()` that calls `invoke('ping')`, `scripts/router.ts`, `styles/main.css` with
`@import "tailwindcss"` and the custom palette (`--color-*: initial`, `--spacing: 1px` — copy
the theme block from `WPT.Dashboard`), `stores/`, `tests/setup.ts`, `types/`, and
`scripts/generated/` (populated by §7).

`vite.config.ts` — from `vite-plus`, and the two corrections from AGENTS.md *Durable failure
shapes* are non-negotiable:

```ts
import { defineConfig } from 'vite-plus'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'
import VueRouter from 'vue-router/vite'

const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [VueRouter({ routesFolder: 'src/pages', dts: 'src/types/typed-router.d.ts' }), vue(), tailwindcss()],
  resolve: { alias: { '@': '/src' } },
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_'],            // NOT 'TAURI_ENV_*' — envPrefix is startsWith
  server: {
    port: 1420, strictPort: true, host: host || false,
    watch: { ignored: ['**/src-tauri/**', '**/crates/**', '**/target/**'] },
  },
  build: {
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: 'oxc',                                // 'esbuild' is deprecated on Vite 8
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  test: { environment: 'jsdom', setupFiles: ['src/tests/setup.ts'], include: ['src/**/*.test.ts'] },
  lint: { options: { typeAware: true, typeCheck: true } },   // plus vite-plus/oxlint-plugin, per its docs
  fmt: { singleQuote: true, semi: false },
  run: {
    tasks: {
      dev:       { command: 'tauri dev', cache: false },
      typecheck: { command: 'vue-tsc --noEmit -p tsconfig.app.json' },
      build:     { command: 'tauri build', cache: false, dependsOn: ['typecheck'] },
      verify:    { command: 'node scripts/verify-prod-bundle.mjs', cache: false },
      types:     { command: 'cargo test -p repo-scan --features typescript', cache: false },
      rust:      { command: 'cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test', cache: false },
    },
  },
})
```

Look up the exact `lint` key for enabling `vite-plus/oxlint-plugin` rather than guessing it
(AGENTS.md, *External docs*). `vp check` is the built-in fmt + lint + tsgolint pass; Vue SFC
type-checking is the `typecheck` task, which `build` depends on.

`scripts/verify-prod-bundle.mjs`: `src/main.ts` contains
`if (import.meta.env.DEV) console.debug('__DEV_BUILD__')`; the script reads `dist/assets/*.js`
and exits non-zero if the sentinel string is present. This is the assertion behind the
`NODE_ENV` + `--mode production` trap. If it fails after a `vp run build`, set
`NODE_ENV=production` explicitly in the `build` task command (the `env` task field only affects
cache keys); do not weaken the check.

`tsconfig.app.json`: `"vueCompilerOptions": { "strictTemplates": true }`, `paths` for `@/*`,
`types` for `vite-plus/client`.

## 6. Rust workspace

Root `Cargo.toml` (new):

```toml
[workspace]
resolver = "3"
members = ["crates/repo-scan", "src-tauri"]

[workspace.package]
edition = "2024"
rust-version = "1.85"           # set by gix
publish = false

[workspace.dependencies]
# every version exact — see PLAN.md §3.1
tauri = "=2.11.5"
gix = "=0.87.1"                 # default features only
ignore = "=0.4.33"
rayon = "=1.12.0"
notify = "=8.2.0"
notify-debouncer-full = "=0.7.0"
tokio = { version = "=1.53.1", features = ["rt-multi-thread", "sync", "time"] }
serde = { version = "=1.0.229", features = ["derive"] }
serde_json = "=1.0.151"
thiserror = "=2.0.20"
anyhow = "=1.0.104"
tracing = "=0.1.44"
tracing-subscriber = "=0.3.23"
ts-rs = { version = "=12.0.1", features = ["serde-compat"] }
dunce = "=1.0.5"
tempfile = "=3.27.0"
tauri-build = "=2.6.3"          # build-dependency of src-tauri
tauri-plugin-dialog = "=2.7.3"
tauri-plugin-store = "=2.4.4"
tauri-plugin-opener = "=2.5.5"
tauri-plugin-window-state = "=2.4.1"

[profile.release]
lto = true
codegen-units = 1
opt-level = 3                   # not "s": this is a scanner
strip = true
# panic stays "unwind": per-repo catch_unwind turns gix panics into RepoStatus.error
```

Then:

- `mv src-tauri/Cargo.lock Cargo.lock` — the lock lives at the workspace root.
- `src-tauri/Cargo.toml`: `edition.workspace = true`, every dep `{ workspace = true }`, add
  `repo-scan = { path = "../crates/repo-scan" }`, **keep `[lib] name = "repo_viewer_lib"`**
  (Windows lib/bin collision), delete any `[profile.*]` block the template put there.
- `crates/repo-scan/Cargo.toml`: `[features] typescript = ["dep:ts-rs"]`; `ts-rs` optional;
  `tempfile` under `[dev-dependencies]`. No `tauri` anywhere in this crate, ever.
- `crates/repo-scan/src/`: `lib.rs`, `error.rs` (a `thiserror` enum and `pub type Result<T>`),
  `model.rs` with a first `RepoStatus` per [PLAN.md §8.1](../PLAN.md) — `gix`-free,
  `#[serde(rename_all = "camelCase")]`, `#[cfg_attr(feature = "typescript", derive(ts_rs::TS), ts(export))]`.
- `.cargo/config.toml` (new, root):

  ```toml
  [env]
  TS_RS_EXPORT_DIR = { value = "src/scripts/generated", relative = true }
  ```

  `relative = true` resolves against the parent of `.cargo/`, i.e. the repo root, so
  `cargo test --features typescript` writes `src/scripts/generated/RepoStatus.ts` from any cwd.
  If the file does not appear there, fall back to `#[ts(export_to = "...")]` on each type.
- `src-tauri/src/lib.rs`: builder with the four plugins (`tauri_plugin_dialog::init()` etc.), a
  `ping` command returning `"pong"`, `.invoke_handler(tauri::generate_handler![ping])`.
- `src-tauri/capabilities/default.json`: `"permissions": ["core:default"]` — nothing else.
  Plugins are called from Rust, so no plugin permission is declared.
- `src-tauri/tauri.conf.json`:
  `build.beforeDevCommand = "vp dev"`, `build.beforeBuildCommand = "vp build --mode production"`,
  `build.devUrl = "http://localhost:1420"`, `build.frontendDist = "../dist"`,
  `app.withGlobalTauri = false`, `app.security.csp` and the `bundle` block exactly as in
  [PLAN.md §9](../PLAN.md), `bundle.targets = ["nsis", "msi"]`.
- Root `.gitignore`: `/target/`, `node_modules/`, `/dist/`, `/src-tauri/gen/schemas/`, plus
  whatever the template's `src-tauri/.gitignore` lists beyond `/target/`. Delete the `/target/`
  entry from `src-tauri/.gitignore`: with the workspace at the root it matches nothing there.

## 7. Generated types

`vp run types` → `src/scripts/generated/RepoStatus.ts` exists and is **committed**. `ipc.ts`
imports from it. CI regenerates and fails on a diff (Phase 8).

## 8. Definition of done

Every line passes on the dev machine; record anything that did not in the PR description.

1. `cargo --version` works (§0), and `vp run rust` is green: fmt, clippy `-D warnings`, tests.
2. `vp check` is green; `vp test run` runs at least one Vue component test through
   `@vue/test-utils` and one `ipc.ts` test through `mockIPC` from `@tauri-apps/api/mocks`.
3. `vp run typecheck` is green on TS 6.0.3 with `vue-tsc` 3.3.11.
4. `vp run types` writes `src/scripts/generated/RepoStatus.ts`; a second run produces no diff.
5. `vp run dev` opens a window; the page calls `ping()` and renders `pong`.
6. **Decide `@typescript/native-preview`.** Put a deliberate type error in a `.ts` file and run
   `vp check`. If tsgolint's `typeCheck: true` reports it, the separate `tsgo` package is
   redundant — remove it from the catalog and from [PLAN.md §3.2](../PLAN.md) and AGENTS.md.
   If not, keep it and add a `tsgo --noEmit` step to the `typecheck` task.
7. `vp run build` produces `src-tauri/target/release/bundle/nsis/*.exe` and `msi/*.msi`, and
   `vp run verify` passes — the sentinel is absent, so `import.meta.env.PROD` is `true`.
8. `pnpm -v` inside the repo prints `10.34.5`; `node -v` prints `v24.20.0`.
9. Docs updated in the same pass: README *To build it* reflects reality; AGENTS.md *Commands*
   lists `vp run typecheck|types|verify|rust` as they exist; PLAN.md status line reads
   "scaffolded" and §11 Phase 0 points at nothing (this file is deleted).
10. Prepared for the user, not run: `git add -A && git commit -m "Phase 0: scaffold"`.
