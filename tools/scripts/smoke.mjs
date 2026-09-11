/**
 * Launches the built binary and asserts it is still alive a few seconds later.
 *
 *     vp run smoke         # or: node tools/scripts/smoke.mjs
 *
 * This is the per-platform launch check PLAN §10.4 asks for, at the only fidelity a GUI binary
 * allows. There is nothing to assert _about_ the webview from outside the process — but a missing
 * WebView2 runtime, a bundle that did not build, a panic in `run()`, and a plugin that fails to
 * register all kill the process within the first second or two, and that is the regression class
 * worth catching in CI rather than by a user.
 *
 * It deliberately runs the **bare executable** rather than an installed copy: `vp run build`
 * produces it either way, it needs no installer to have run, and it is the same binary the bundle
 * wraps. On Linux there is no display on a CI runner, so the workflow puts `xvfb-run` in front of
 * this; without one the app exits immediately and the check fails, which is the honest answer.
 *
 * A clean exit is asserted after the kill because a process that ignores termination is its own
 * defect — the window close path unwinds a poll thread, a watcher and a refresh thread, and one of
 * them hanging is exactly the kind of thing that only shows up under a real launch.
 */
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * How long the process must stay up to count as having started.
 */
const ALIVE_MS = 5000;

/**
 * How long it then gets to go away after being asked to.
 */
const EXIT_MS = 10_000;

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const binary = join(
  repoRoot,
  'target',
  'release',
  process.platform === 'win32' ? 'repo-viewer.exe' : 'repo-viewer',
);

if (!existsSync(binary)) {
  console.error(`No binary at ${binary}. Run \`vp run build\` first.`);
  process.exit(1);
}

const child = spawn(binary, [], { stdio: ['ignore', 'pipe', 'pipe'] });

let output = '';
child.stdout.on('data', (chunk) => (output += chunk));
child.stderr.on('data', (chunk) => (output += chunk));

/**
 * Resolves with the exit result, or null if the process is still running when `ms` elapses.
 */
function settledWithin(ms) {
  return new Promise((resolveWith) => {
    const timer = setTimeout(() => resolveWith(null), ms);
    child.once('exit', (code, signal) => {
      clearTimeout(timer);
      resolveWith({ code, signal });
    });
  });
}

const died = await settledWithin(ALIVE_MS);
if (died) {
  console.error(
    `The app exited ${Math.round(ALIVE_MS / 1000)}s into startup ` +
      `(code ${died.code}, signal ${died.signal}). It should still be running.\n` +
      (output.trim() || '(the process wrote nothing)'),
  );
  process.exit(1);
}

child.kill();

const stopped = await settledWithin(EXIT_MS);
if (!stopped) {
  child.kill('SIGKILL');
  console.error(
    `The app was still running ${Math.round(EXIT_MS / 1000)}s after being asked to stop.\n` +
      (output.trim() || '(the process wrote nothing)'),
  );
  process.exit(1);
}

console.info(`Launch verified: the app ran for ${Math.round(ALIVE_MS / 1000)}s and then exited.`);
