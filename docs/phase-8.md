# Phase 8 — Packaging

Runbook for the final phase of [PLAN.md §11](../PLAN.md). It is written so a session with no prior
context can run it top to bottom. It is deleted when Phase 8 is done: everything durable in it ends
up in [README.md](../README.md) (how a user gets the app), [AGENTS.md](../AGENTS.md) (commands and
traps), and PLAN.md §11's Phase 8 entry.

Read [AGENTS.md](../AGENTS.md) first. Two of its rules bind every step here: **Git is read-only for
agents** — prepare commands, do not commit, tag, or push — and **every dependency version is exact**.

## What this phase is, and what it is not

Phase 8 makes the app installable by someone who is not going to build it. That is four separate
things, and only the first is about the installer:

1. The app can say why it will not start on a machine with no WebView2 runtime.
2. The installer looks like this application rather than like the Tauri template, and installing a
   new version over an old one works.
3. Something other than a developer's own machine builds it.
4. There is a place to get it from.

**There is no in-app updater.** Distribution is a GitHub Release per tag; a user downloads the
installer and runs it over the top. `tauri-plugin-updater`, a minisign keypair, a hosted manifest and
a baked-in endpoint are all out of scope, and deliberately so — the endpoint and public key are
compiled into every build, so the first release would fix them permanently.

**Signing is deferred.** PLAN §9 makes it a prerequisite for goal 1 and that has not changed: an
unsigned installer handed to a colleague on a corporate-managed machine does not get a click-through, it
gets blocked. What changed is the ordering — everything here completes without it, and adding it
later is a config block plus a secret. See §7.

---

## 0. Gate: the toolchain must execute

Application allowlisting on this machine denies execution of freshly installed binaries even when
ACLs are correct. It has bitten the Rust toolchain three separate times, and `cargo --version`
succeeding proves nothing about `rustc`, because they are blocked independently.

```sh
cargo --version && rustc -vV | head -3 && node --version
```

All three print → continue. Any prints `Access is denied (os error 5)` or exits `0xc0000022` → the
block is back; ask for ThreatLocker learning mode before assuming an IT ticket. Note that the
ambient `node` on this machine is 22 via nvm-windows while `.node-version` pins **24.20.0** — `vp`
uses its own pinned copy, so `vp run …` is correct and a bare `node tools/scripts/foo.mjs` may not be.

This phase also **builds installers**, which creates and runs fresh executables under `target/`. If
the allowlisting is in enforcing mode, expect it to object to the built `repo-viewer.exe` before it
objects to anything else.

---

## 1. Icons

The fifteen files in `src-tauri/icons/` are still the Tauri template's. Replace them from one source.

`tauri icon` accepts a squared SVG with transparency, so there is no raster source to keep in sync
and no image-processing dependency to add.

1. Author `src-tauri/icons/source.svg`, 1024×1024, transparent. Palette comes from
   `src/styles/theme.css` — `--color-primary-500` is `hsl(215 42% 50%)`, a navy at hue 215 / 42%
   saturation, with `-200` and `-750` as the tints. It has to read at 32px, so no fine detail and no
   text.
2. Generate:

   ```sh
   pnpm exec tauri icon src-tauri/icons/source.svg
   ```

   This overwrites all fifteen files in place. `bundle.icon` in `tauri.conf.json` already names the
   five it needs; no config change.

3. Keep `source.svg` committed beside the generated set, or the next regeneration starts from a PNG
   someone traced back out of the `.ico`.

**Check:** `src-tauri/icons/128x128.png` is no longer the Tauri logo, and `vp run build` produces an
installer whose window and Add/Remove Programs entry carry the new icon.

---

## 2. WebView2 runtime probe

PLAN §10.1 requires the binary to read the WebView2 Runtime's `pv (REG_SZ)` value under **both**
`HKEY_LOCAL_MACHINE` and `HKEY_CURRENT_USER` before creating the window, so a missing runtime
produces a native message box naming the fix rather than a blank exit.

New module `src-tauri/src/webview2.rs`. Declared in `lib.rs`'s alphabetical `mod` block after
`mod stream;` and called from `run()` **immediately after `init_tracing()`, before
`tauri::Builder::default()`** — windows declared in `tauri.conf.json` are created during `build()`,
so `setup` is already too late.

### Shape

Split the registry read from the decision, the way `crates/repo-scan/src/exe.rs` splits
`resolve_program` from `resolve_in`, so the decision half is unit-testable off Windows:

- `fn installed(hklm: Option<&str>, hkcu: Option<&str>) -> bool` — pure.
- `#[cfg(windows)] fn read_pv() -> (Option<String>, Option<String>)` — reads both locations.
- `fn ensure()` — the entry point; a no-op off Windows.

### Three things that make it wrong if missed

**A present key does not mean a present runtime.** Microsoft's detection contract is that the value
is absent, null, empty, **or `0.0.0.0`** when the runtime is not installed. A check that only asks
whether the read succeeded passes on a machine with no runtime.

**A 64-bit process cannot see the registration through the obvious path.** The Edge updater
registers 32-bit, so a 64-bit build must name `WOW6432Node` explicitly:

```
HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}
HKCU\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}
```

Reading `HKLM\SOFTWARE\Microsoft\EdgeUpdate\...` from a 64-bit process returns nothing on a machine
that has the runtime. Fall back to the non-WOW path for a 32-bit build.

**There is no console to print to.** `main.rs` sets `windows_subsystem = "windows"` in release, so a
`eprintln!` goes nowhere. The message box is the only channel, which is why PLAN specifies one.

### Dependencies

First `[target.'cfg(windows)'.dependencies]` section in the repo. Exact-pin in the root
`[workspace.dependencies]`, reference as `{ workspace = true }`:

| Crate         | Pin       | For                                                   |
| ------------- | --------- | ----------------------------------------------------- |
| `winreg`      | `=0.55.0` | the registry read, in safe Rust                       |
| `windows-sys` | `=0.61.2` | `MessageBoxW`, feature `Win32_UI_WindowsAndMessaging` |

Both versions are **already in `Cargo.lock`** transitively. Matching them matters: the tree already
carries four `windows-sys` majors, and picking a different one adds a fifth for nothing.

Gate with `#[cfg(windows)]`, not `target_os`, and put the platform body in an inner
`#[cfg(windows)] { … }` block so the non-Windows build compiles to a no-op — the pattern
`open.rs::set_console` and `fetch.rs::hide_console` already use, `expect` rather than `allow` on the
unused-parameter arm.

**Check:** unit tests cover absent / empty / `0.0.0.0` / present. The negative path cannot be staged
on this machine — WebView2 is inbox on Windows 11 — so confirm the box appears by temporarily
pointing the lookup at a wrong GUID and running the release binary. When checking the key by hand
under MSYS, `reg query … //v pv` is the correct spelling; `/v` gets rewritten.

---

## 3. Installer configuration

In `src-tauri/tauri.conf.json`, under `bundle.windows`:

```jsonc
"wix": { "upgradeCode": "<a GUID generated once, then never changed>" },
"nsis": { "installMode": "currentUser" }
```

**Why `upgradeCode` is pinned.** Tauri derives it from `productName`. MSI performs a major upgrade
only when the UpgradeCode is stable and the version increments — so renaming the product later would
silently convert every upgrade into a second side-by-side install. One line, closed permanently.

**Why `installMode` is explicit.** `currentUser` is the current default. The NSIS template records
the mode and matches against it on upgrade, so an implicit default that moves in a future Tauri
release breaks upgrade detection on machines that already have the app.

**Installing over the top already works, and it is worth knowing how.** The NSIS template reads
`DisplayVersion` from `…\CurrentVersion\Uninstall\<ProductName>`, semver-compares against the
incoming build, and shows a page offering to remove the old version first — silently under `/P`. A
user double-clicks the new `.exe` and clicks through one extra page. No manual uninstall.

### The second, offline installer

PLAN §9 and §10.1 want a second Windows artifact for egress-blocked fleets. New
`src-tauri/tauri.offline.conf.json` holding only the override:

```json
{ "bundle": { "windows": { "webviewInstallMode": { "type": "offlineInstaller" } } } }
```

Built with `tauri build --config src-tauri/tauri.offline.conf.json`; `--config` merges with the base
rather than replacing it.

**Both builds emit identical filenames.** Run one after the other and the second silently overwrites
the first. Rename the bootstrapper artifacts before building the offline pair — the release workflow
does this, and anyone building both by hand has to as well.

---

## 4. Version consistency

`package.json`, `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml` each carry a version, and
nothing makes them agree. They feed different things — the installer filename, the Add/Remove
Programs entry, the crate — so they can disagree and no build fails.

`tools/scripts/check-versions.mjs` asserts all three match, and additionally that a `GITHUB_REF_NAME`
tag matches when one is set. `crates/repo-scan` is exempt: it is an internal `publish = false`
library whose version means nothing.

Wire as a `versions` task in `vite.config.ts`'s `run.tasks` — object form, `cache: false`, with a
`//` comment saying why, matching every other entry. The `tools/scripts/**` lint override already
exempts the path from `no-console` and `unicorn/no-process-exit`.

**Check:** passes as-is; hand-edit one version and confirm it fails and names both files.

---

## 5. Launch smoke test

`tools/scripts/smoke.mjs` spawns the built binary, waits ~5 s, asserts the process is still alive,
then terminates it and asserts it went away. Wired as a `smoke` task.

That is §10.4's "assert the webview initializes" at the only fidelity a GUI binary allows: a missing
runtime, a broken bundle or a panic in `run()` all kill the process inside that window, and that is
the regression class this exists for. Linux needs `xvfb-run` in front of it.

---

## 6. CI and releases

**PLAN §10.4 specifies Azure DevOps. That is wrong for this repo** — it lives at
`github.com/b-meyer/repo-viewer`. CI is GitHub Actions; §10.4 is corrected to match in the documentation pass below.

Both workflows install Node with `actions/setup-node` and `node-version-file: '.node-version'`, then
`npm i -g pnpm@12.3.4`, `pnpm install --frozen-lockfile`, and reach the toolchain as `pnpm exec vp …`
because `vp` is not global on a runner.

### `.github/workflows/ci.yml` — push and PR to `main`

| Job      | Runner                                           | Runs                                                                                                                     |
| -------- | ------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| `checks` | `windows-latest`                                 | `vp check`, `vp run typecheck`, `vp test run`, `vp run versions`, `vp run rust`, `vp run types` + `git diff --exit-code` |
| `build`  | `windows-latest`, `macos-latest`, `ubuntu-22.04` | `vp run build`, `vp run verify`, `vp run smoke`                                                                          |

The `git diff --exit-code` over `src/scripts/generated/` after `vp run types` is the thing that
actually stops the committed bindings drifting from `model.rs`.

`ubuntu-22.04` is the runner image, not a container — it satisfies §10.1's glibc floor on its own.
Linux installs `libwebkit2gtk-4.1-dev build-essential libssl-dev librsvg2-dev libxdo-dev` first.
`Swatinem/rust-cache` on every leg.

Per §10.4, **the Windows leg is the pipeline and the other two are proof of portability.** Do not let
a red Linux leg block a Windows release.

### `.github/workflows/release.yml` — `push: tags: ['v*']`

`permissions: contents: write`. Same three legs; Windows builds twice, bootstrapper then offline,
renaming between. `softprops/action-gh-release` attaches every artifact, with a body that says the
build is unsigned and what that means on a managed Windows machine.

---

## 7. Signing — deferred, not dropped

Not done in this phase. Two things have to be true before it can be, and neither is code:

1. **An Authenticode certificate trusted by the tenant.** This is an IT request with lead time, and
   it should be filed now rather than when the rest of this is finished.
2. **Somewhere for the key to live.** This repo is public and on a personal account. GitHub Actions
   secrets are technically safe there — fork PRs never receive them — but a corporate certificate in a
   personal public repo is IT's call, not ours. The alternatives are signing locally and attaching
   manually, or moving the repo into a company-owned org first.

**File the request naming a ThreatLocker publisher rule, not just the certificate.** ThreatLocker is
the lever that denies freshly written binaries on this machine — Defender for Endpoint is resident
with tamper protection, but it is not what blocks these — and it allowlists by publisher, hash or
path. A signed build that nobody wrote a rule for is still blocked, and a hash rule has to be
repeated per build, so "we'll sign it" is not by itself the ask.

Ask for the installer's plugin too. Tauri's NSIS installer extracts `nsis_tauri_utils.dll` to
`%TEMP%` and loads it, and that DLL is denied on its own account — approving the installer does not
approve it.

When it lands, the change is `bundle.windows.certificateThumbprint`, `digestAlgorithm: "sha256"` and
a `timestampUrl`, plus the secret. Leave the block shaped for it.

---

## 8. Documentation, same pass

Per the documentation rules — describe the design, do not narrate the change.

- **PLAN §10.4** — rewrite for GitHub Actions and GitHub Releases.
- **PLAN §9** — the deferred-signing position; the upgrade and UpgradeCode facts.
- **PLAN §12 item 5** — settle it: GitHub Release per tag, manual install, no updater.
- **PLAN §11** — the Phase 8 _Verified_ / _Settled by writing it_ entry.
- **AGENTS.md** — `vp run versions` and `vp run smoke` rows; the new durable failure shapes.
- **README** — where releases live, that installing over the top works, the unsigned caveat.

One existing defect to correct while in there: AGENTS.md's production-build trap says to specify
`vp build --mode production` as `beforeBuildCommand`. It is `node tools/scripts/build-frontend.mjs`,
which wraps it — setting the variable inline is not portable, because Tauri spawns that command
through `cmd.exe` on Windows.

---

## 9. Done when

1. `vp check`, `vp run typecheck`, `vp test run`, `vp run rust`, `vp run versions` all clean.
2. `vp run types` produces no diff, twice running.
3. `vp run build` then `vp run verify` then `vp run smoke` pass.
4. The probe's unit tests cover absent / empty / `0.0.0.0` / present, and the message box has been
   seen once for real.
5. **The upgrade path is proven, not assumed.** Install `0.1.0`; bump to `0.1.1`; run the new
   installer over the top. No manual uninstall, one entry in Add/Remove Programs at the new version,
   and `settings.json` plus `cache.json` in `%APPDATA%` survived. Repeat for the MSI — that is the
   only thing that exercises the pinned UpgradeCode.
6. The offline installer builds and is ~250 MB larger (measured; Tauri documents ~127 MB).
7. CI is green on a branch, and a tag produces a Release with the expected artifacts attached.
8. `vp run bundles` passes on every platform leg — a build that emits no installer looks healthy to
   every other check in the pipeline.

**Only item 7 is outstanding.** Everything above it has been done and checked; the workflows cannot
be exercised from a working copy, so they are the one part of this phase that has never run. Two
things to watch on that first run, both of which would show up nowhere else:

- `tauri build`'s `beforeBuildCommand` shells out to `vp` from inside the build. On a runner `vp`
  exists only in `node_modules/.bin`, so this depends on `pnpm exec` having put that on `PATH` for
  the whole process tree. It works locally because `vp` is global here, which means locally proves
  nothing about it.
- The `git diff --exit-code` after `vp run types` on the Windows leg. Line-ending normalisation is
  the thing that would make it report a diff that is not a diff. **This one landed**, one step
  earlier than expected: `vp check` failed all 93 files on the first run, because a Windows checkout
  is CRLF and oxfmt formats to LF. `.gitattributes` pins the working tree to LF; see _Durable
  failure shapes_ in [AGENTS.md](../AGENTS.md).

Delete this file once that run is green.
