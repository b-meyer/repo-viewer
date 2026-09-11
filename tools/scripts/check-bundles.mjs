/**
 * Asserts that the build produced the installers this platform is supposed to produce.
 *
 *     vp run bundles       # or: node tools/scripts/check-bundles.mjs
 *
 * `tauri build` is happy to produce **no installer at all**. Ask it for a bundle target that does
 * not apply to the host — `nsis` on Linux, say — and it compiles the binary, skips bundling, exits
 * zero and says nothing. `bundle.targets` was `["nsis", "msi"]` for a long time, which meant every
 * macOS and Linux build silently emitted nothing, and the only thing that ever looked in
 * `target/release/bundle/` was the release workflow's staging step. So the first symptom was a
 * release failing to find its own artifacts, months after the config was wrong.
 *
 * Nothing else closes that gap. `vp run verify` inspects `dist/`, which is the frontend bundle and
 * is produced either way; `vp run smoke` runs the bare executable, which is also produced either
 * way. A build emitting zero installers is indistinguishable from a healthy one unless something
 * looks for the installers by name.
 *
 * AppImage is the one target whose absence is not a failure. PLAN §9 has it as best-effort because
 * of recurring upstream packaging bugs, so it is reported and the check still passes.
 */
import { existsSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const bundleDir = join(repoRoot, 'target', 'release', 'bundle');

/**
 * What each platform must emit, as a bundle subdirectory and the file suffix inside it.
 *
 * Keyed by `process.platform`. `required: false` means "report it, do not fail" — see the AppImage
 * note above.
 */
const EXPECTED = {
  win32: [
    { dir: 'nsis', suffix: '-setup.exe', required: true },
    { dir: 'msi', suffix: '.msi', required: true },
  ],
  darwin: [{ dir: 'dmg', suffix: '.dmg', required: true }],
  linux: [
    { dir: 'deb', suffix: '.deb', required: true },
    { dir: 'appimage', suffix: '.AppImage', required: false },
  ],
};

/**
 * Lists the files in one bundle subdirectory that end with the given suffix.
 *
 * A missing directory is the same answer as an empty one — Tauri does not create a directory for a
 * target it did not build.
 *
 * @param dir - Subdirectory of `target/release/bundle`.
 * @param suffix - The filename ending that identifies this bundle type.
 * @returns Matching filenames, possibly empty.
 */
function found(dir, suffix) {
  const path = join(bundleDir, dir);
  if (!existsSync(path)) return [];
  return readdirSync(path).filter((name) => name.endsWith(suffix));
}

const expected = EXPECTED[process.platform];
if (!expected) {
  console.error(`No expected bundles recorded for platform "${process.platform}".`);
  process.exit(1);
}

if (!existsSync(bundleDir)) {
  console.error(
    `No bundle directory at ${bundleDir}.\n` +
      'Run `vp run build` first. If it did run, the build produced no installers at all — check ' +
      '`bundle.targets` in src-tauri/tauri.conf.json.',
  );
  process.exit(1);
}

const missing = [];
for (const { dir, suffix, required } of expected) {
  const files = found(dir, suffix);
  if (files.length > 0) {
    for (const name of files) console.info(`  ${dir}/${name}`);
  } else if (required) {
    missing.push(`${dir}/*${suffix}`);
  } else {
    console.info(`  ${dir}/*${suffix} — absent (best-effort target, not a failure)`);
  }
}

if (missing.length > 0) {
  console.error(
    `\nThe build emitted no ${missing.join(' and no ')} on ${process.platform}.\n` +
      'A `tauri build` that is asked for targets the host cannot produce exits zero having ' +
      'bundled nothing, so this is the only thing that catches it. Check `bundle.targets` in ' +
      'src-tauri/tauri.conf.json — "all" selects the applicable targets per platform.',
  );
  process.exit(1);
}

console.info(`Bundles present for ${process.platform}.`);
