/**
 * Asserts that `dist/` is a genuine production build.
 *
 *     vp run verify        # or: node tools/scripts/verify-prod-bundle.mjs
 *
 * `src/main.ts` contains `if (import.meta.env.DEV) console.debug('__DEV_BUILD__')`. In a production
 * build Vite replaces `import.meta.env.DEV` with `false` and the minifier eliminates the dead
 * branch, taking the sentinel string with it. If the sentinel is still in the emitted JavaScript,
 * `DEV` was true — the bundle is a development build wearing a production filename, and every
 * `import.meta.env.PROD` guard in the app is inverted.
 *
 * That failure is silent by nature: the app runs, looks right, and only misbehaves where an env
 * guard mattered. This check is the only thing that catches it. If it fails, fix the build (see
 * `tools/scripts/build-frontend.mjs`) rather than relaxing the assertion.
 */
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const SENTINEL = '__DEV_BUILD__';
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const assetsDir = join(repoRoot, 'dist', 'assets');

let entries;
try {
  entries = readdirSync(assetsDir);
} catch {
  console.error(`No bundle at ${assetsDir}. Run \`vp run build\` first.`);
  process.exit(1);
}

const scripts = entries.filter((name) => name.endsWith('.js'));
if (scripts.length === 0) {
  console.error(`No JavaScript emitted into ${assetsDir}. The build did not produce a bundle.`);
  process.exit(1);
}

const offenders = scripts.filter((name) =>
  readFileSync(join(assetsDir, name), 'utf8').includes(SENTINEL),
);

if (offenders.length > 0) {
  console.error(
    `Not a production build: the \`${SENTINEL}\` sentinel survived in ${offenders.join(', ')}.\n` +
      'That means `import.meta.env.DEV` was true, so `PROD` is false throughout the bundle.\n' +
      'Both NODE_ENV=production and --mode production are required; see tools/scripts/build-frontend.mjs.',
  );
  process.exit(1);
}

console.info(`Production build verified: ${scripts.length} script(s), no dev sentinel.`);
