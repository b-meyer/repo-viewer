# repo-viewer

Point it at a folder and get a live dashboard of every Git repo beneath it — branch,
ahead/behind, dirty state, file counts — without opening each one in an IDE.

IDEs show you uncommitted changes and unpushed commits one repository at a time. With a few
dozen checkouts under `C:\Working\Source`, answering "what have I not pushed?" means opening
every one. This answers it in a single view.

A Tauri 2 desktop app: Rust backend, Vue 3 frontend, native installer, no server.

- **[PLAN.md](./PLAN.md)** — the specification: design decisions, roadmap, open questions.
- **[AGENTS.md](./AGENTS.md)** — conventions, hard rules, and the traps. Read before changing code.

---

## How it fits together

Layers 1–2 are the only JavaScript in the system; layer 3 down is Rust.

```mermaid
flowchart TB
    subgraph L1["1 &nbsp;UI layer &mdash; the only JavaScript in the system"]
        L1A["Root folder picker<br/>@tauri-apps/plugin-dialog 2.7.3"]
        L1B["Repo table, filter chips, search<br/>vue 3.5.42 &middot; pinia 4.0.3 &middot; tailwindcss 4.3.3"]
        L1C["Detail drawer<br/>requests Tier 2 on open"]
    end

    subgraph L2["2 &nbsp;IPC bridge &mdash; @tauri-apps/api 2.11.1"]
        L2A["invoke &mdash; commands down"]
        L2B["Channel&lt;ScanEvent&gt; &mdash; results up, ordered and batched"]
        L2C["listen &mdash; low-frequency notices only"]
    end

    subgraph L3["3 &nbsp;Tauri core &mdash; tauri 2.11.5"]
        L3A["invoke_handler command registry<br/>own commands need no capability declaration"]
    end

    subgraph L4["4 &nbsp;Discovery &mdash; ignore 0.4.33"]
        L4A["WalkBuilder::build_parallel<br/>genuinely parallel descent"]
        L4B["prune node_modules, target, .venv<br/>resolve .git as file vs dir<br/>same_file_system, max_depth"]
    end

    subgraph L5["5 &nbsp;Git reads &mdash; gix 0.87.1, fanned out by rayon 1.12.0, zero C dependencies"]
        L5A["Tier 0 &mdash; refs only, sub-ms per repo<br/>head_ref &middot; rev_walk.with_boundary &middot; commit_graph"]
        L5B["Tier 1 &mdash; dirty flag<br/>is_dirty, early exit"]
        L5C["Tier 2 &mdash; full counts, lazy<br/>status, index-to-worktree diff"]
    end

    subgraph L6["6 &nbsp;Watching &mdash; notify 8.2.0"]
        L6A["ONE watcher over N git dirs<br/>notify-debouncer-full 0.7.0, 300-500 ms<br/>60 s poll and focus-refresh as the safety net"]
    end

    subgraph L7["7 &nbsp;Side channels &mdash; subprocess and OS"]
        L7A["git CLI subprocess<br/>fetch / pull / push ONLY<br/>real credential helpers, SSH config, proxies"]
        L7B["tauri-plugin-opener 2.5.5<br/>editor &middot; terminal &middot; file manager"]
        L7C["tauri-plugin-store 2.4.4<br/>JSON cache of roots and last status"]
    end

    OUT["Rows paint progressively, tier by tier, back up through Channel to the repo table.<br/>Tiers not yet computed render as unknown &mdash; never as 0."]

    L1 --> L2
    L2 --> L3
    L3 --> L4
    L3 --> L7
    L4A --> L4B
    L4 -->|"repo paths"| L5
    L4 -->|"register git dirs"| L6
    L5A --> L5B
    L5B --> L5C
    L5 -.-> OUT
    L6 -.-> OUT
    L7A -.->|"updates last_fetched"| L5

    style OUT fill:#eef7ee,stroke:#8bb88b
```

---

## Requirements

### To run it

Nothing. The frontend is bundled into the native binary — no Node, no Rust.

| Platform | Notes |
|---|---|
| Windows 11 | WebView2 is inbox; nothing to install |
| Windows 10 1803+ | WebView2 present on almost all devices; the installer's bootstrapper covers the rest |
| macOS 10.15+ | WKWebView is part of the OS |
| Ubuntu 22.04+ / Debian 12+ | `apt` pulls `libwebkit2gtk-4.1-0` from the `.deb` |

Ubuntu 20.04 and Debian 11 are **not supported** — Tauri 2 needs webkit2gtk **4.1**, which those
releases do not ship. See [PLAN.md §10](./PLAN.md) for the full platform matrix.

The `git` CLI is needed only for fetch/pull/push. Viewing status works without it; those actions
disable themselves with an explanation if it is absent.

### To build it

| | |
|---|---|
| Node.js | 22.13+ — corepack fetches pnpm from `packageManager` |
| Rust | via `rustup`, MSVC host (`x86_64-pc-windows-msvc`). MSRV **1.85**, set by `gix` |
| Windows | VS C++ build tools — `MSVC v… C++ x64/x86 build tools (Latest)` + `Windows 11 SDK`. Nothing else from the C++ workload is needed |
| macOS | Xcode Command Line Tools |
| Linux | `libwebkit2gtk-4.1-dev`, `build-essential`, `libssl-dev`, `librsvg2-dev`, `libxdo-dev` |

Verify a Windows toolchain with a real link, not a version print:

```sh
cargo new --bin /tmp/linkcheck && cd /tmp/linkcheck && cargo build
```

`@tauri-apps/cli` is a project dependency, not a global install. The Tauri bundler downloads WiX
and NSIS on the first `tauri build`, so that build needs network access.

---

## Getting started

```sh
pnpm install            # or: vp install
```

`vp` is the entry point for every command — it fronts Vite, Vitest, oxlint, oxfmt, and the task
runner. Never call `pnpm` / `npm` / `yarn` scripts directly; see [AGENTS.md](./AGENTS.md).

| Task | Command |
|---|---|
| Dev (Vite + Tauri window, hot reload) | `vp run dev` |
| Frontend only, in a browser | `vp dev` |
| Type-check, lint, format, test | `vp check` |
| Fix what is auto-fixable | `vp check --fix` |
| Unit tests | `vp test run` |
| Production build + installer | `vp run build` |
| Rust checks | `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` |
| Scan a tree without the GUI | `cargo run --release --example scan -- C:/Working/Source` |

---

## Layout

```
repo-viewer/
├── Cargo.toml                    # [workspace] + [workspace.dependencies]
├── package.json                  # one package; vp fronts every command
├── pnpm-workspace.yaml           # catalog: only — every version exact, declared once
├── vite.config.ts                # vp config: vite + test + lint + fmt + run.tasks
├── azure-pipelines.yml
│
├── src/                          # ── Vue frontend
│   ├── layout/                   # App.vue shell, Header
│   ├── pages/                    # file-based routes; index.vue = /
│   ├── components/
│   │   ├── inputs/               # App*.vue reka-ui wrappers — always use at call sites
│   │   ├── repos/                # RepoTable, RepoRow, FilterBar, DetailDrawer …
│   │   └── feedback/             # AppToaster, ScanProgress
│   ├── scripts/                  # ipc.ts, scan.ts, search.ts, router.ts, utils.ts
│   │   └── generated/            # ts-rs output, committed
│   ├── stores/                   # Pinia: repos, filters, settings
│   ├── styles/                   # main.css, theme.css (custom palette)
│   ├── tests/                    # shared harness only — unit tests sit beside their subject
│   └── types/                    # ambient .d.ts only
│
├── crates/
│   └── repo-scan/                # ── THE ENGINE. Zero Tauri dependency.
│       ├── examples/scan.rs      # run the engine without the GUI
│       ├── src/
│       │   ├── discover/         # parallel walk, prune, .git resolution
│       │   ├── status/           # tier0 / tier1 / tier2 / ahead_behind
│       │   ├── watch/            # one debounced watcher
│       │   ├── fetch.rs          # git CLI subprocess
│       │   ├── model.rs          # RepoStatus and friends
│       │   └── error.rs
│       └── tests/                # fixtures are built into a TempDir, never committed
│
└── src-tauri/                    # ── THIN shell. Tauri glue only.
    ├── tauri.conf.json
    ├── capabilities/             # plugin permissions
    └── src/
        ├── lib.rs                # run(): builder, plugins, state, invoke_handler
        ├── state.rs
        ├── stream.rs             # domain events → tauri::ipc::Channel
        └── commands/             # scan, repo, fetch, config
```

Coming from .NET: there is no `.sln` and no `.csproj`. The root `Cargo.toml` is the workspace
manifest, one `Cargo.toml` per crate replaces project files, and the root `package.json` plus
`pnpm-workspace.yaml` cover the frontend. Visual Studio has weak Rust support — use VS Code with
rust-analyzer, or RustRover.

---

## Licence

Internal to CIT Solutions.
