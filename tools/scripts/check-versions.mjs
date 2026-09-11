/**
 * Asserts that everything claiming to be the app's version agrees.
 *
 *     vp run versions      # or: node tools/scripts/check-versions.mjs
 *
 * Three files carry a version and nothing makes them agree. They feed different things —
 * `package.json` the workspace, `tauri.conf.json` the installer filename and the Add/Remove
 * Programs entry, `src-tauri/Cargo.toml` the crate and the compiled binary's metadata — so they can
 * disagree and no build fails. The result is an installer called 0.1.1 that reports itself as 0.1.0
 * once installed, which is discovered by a user rather than by CI.
 *
 * `crates/repo-scan` is deliberately not checked. It is an internal `publish = false` library whose
 * version is never surfaced anywhere, and making it move in lockstep would be ceremony rather than
 * a guard.
 *
 * When `GITHUB_REF_NAME` is set and looks like a version tag, it is checked too. That is the
 * release workflow's case: tagging `v0.1.1` against a tree that still says `0.1.0` would otherwise
 * publish a release whose assets all carry the wrong number.
 */
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

/**
 * Reads a version out of one file.
 *
 * @param label - How the file is named in a failure message.
 * @param relativePath - Path to the file, relative to the repository root.
 * @param extract - Pulls the version string out of the file's contents.
 * @returns The file's declared version, and the label to blame if it is wrong.
 */
function read(label, relativePath, extract) {
  const contents = readFileSync(join(repoRoot, relativePath), 'utf8');
  const version = extract(contents);
  if (!version) {
    console.error(`Could not find a version in ${relativePath}.`);
    process.exit(1);
  }
  return { label, relativePath, version };
}

/**
 * Pulls the first top-level `version = "..."` out of a Cargo manifest.
 *
 * Anchored to the start of a line so a dependency's `version = "=1.2.3"` further down cannot match
 * — `[package]` comes first in both manifests here, and its key is the only one at column zero.
 *
 * @param contents - The manifest's text.
 * @returns The declared version, or undefined.
 */
function cargoVersion(contents) {
  return /^version = "([^"]+)"/m.exec(contents)?.[1];
}

const declared = [
  read('package.json', 'package.json', (text) => JSON.parse(text).version),
  read('tauri.conf.json', 'src-tauri/tauri.conf.json', (text) => JSON.parse(text).version),
  read('src-tauri/Cargo.toml', 'src-tauri/Cargo.toml', cargoVersion),
];

const [first, ...rest] = declared;
const disagreeing = rest.filter((entry) => entry.version !== first.version);

if (disagreeing.length > 0) {
  console.error('Version mismatch:');
  for (const entry of declared) {
    console.error(`  ${entry.version.padEnd(12)} ${entry.relativePath}`);
  }
  console.error('\nAll three must match. Nothing else enforces this.');
  process.exit(1);
}

// Only a `v`-prefixed tag is a release tag; anything else on GITHUB_REF_NAME is a branch name, which
// this has no opinion about.
const ref = process.env.GITHUB_REF_NAME;
if (ref?.startsWith('v')) {
  const tagged = ref.slice(1);
  if (tagged !== first.version) {
    console.error(
      `Tag ${ref} does not match the declared version ${first.version}.\n` +
        'Bump the three version fields to match the tag, or tag the version that is committed.',
    );
    process.exit(1);
  }
  console.info(`Version ${first.version} agrees across all three files and matches tag ${ref}.`);
} else {
  console.info(`Version ${first.version} agrees across all three files.`);
}
