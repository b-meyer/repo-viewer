/**
 * Frontend production build, invoked by Tauri's `build.beforeBuildCommand`.
 *
 * ## Why this wrapper exists
 *
 * A production bundle needs `NODE_ENV=production` **and** `--mode production`, and neither implies
 * the other. Vite derives `isProduction` from `NODE_ENV` whenever it is already set, which
 * overrides `--mode`. The `vp` task runner sets `NODE_ENV`, and `tauri build` — spawned by `vp run
 * build` — passes its environment down to this command. So a bare `vp build --mode production` as
 * the `beforeBuildCommand` inherits the task runner's `NODE_ENV` and silently emits a bundle where
 * `import.meta.env.DEV` is **true** and `PROD` is **false**, inverting every env guard in the app.
 *
 * Setting the variable in the command string is not portable — `NODE_ENV=x cmd` is POSIX syntax and
 * Tauri runs `beforeBuildCommand` through the platform shell, which is `cmd.exe` on Windows. Doing
 * it here works everywhere and needs no extra dependency.
 *
 * `tools/scripts/verify-prod-bundle.mjs` asserts the result, so this is the fix and that is the
 * regression check. Do not weaken either.
 */
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Resolved from this file rather than from `process.cwd()`. Tauri runs `beforeBuildCommand` from
// the project root, but a hand-run `node tools/scripts/build-frontend.mjs` from anywhere else
// would otherwise build the wrong directory.
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

const result = spawnSync('vp', ['build', '--mode', 'production'], {
  cwd: repoRoot,
  stdio: 'inherit',
  shell: true,
  env: { ...process.env, NODE_ENV: 'production' },
});

if (result.error) {
  console.error(`Failed to start the frontend build: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status ?? 1);
