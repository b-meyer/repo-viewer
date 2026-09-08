//! Builds the discovery fixture tree at test time.
//!
//! A nested `.git` cannot be committed inside the outer repository, and the cases most worth
//! testing — a linked worktree, a submodule, a bare repository — are exactly the ones that need
//! real Git metadata. So the tree is built into a `tempfile::TempDir` by shelling out to `git`, and
//! nothing under `tests/fixtures/` is committed.
//!
//! Two traps are handled here rather than in the tests:
//!
//! - **Local submodules are refused by default.** Since the fix for CVE-2022-39253, `git
//!   submodule add` rejects the `file` transport, which covers a plain local path. Every
//!   invocation therefore passes `-c protocol.file.allow=always`. Without it the clone fails with
//!   a transport error that reads like a bad path.
//! - **The developer's own Git config must not participate.** `GIT_CONFIG_GLOBAL` and
//!   `GIT_CONFIG_SYSTEM` are pointed at a path that does not exist, which Git reads as an empty
//!   config, so `core.autocrlf`, hooks, and templates cannot change the shape of the tree.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::TempDir;

/// Arguments prepended to every `git` invocation.
///
/// Identity is set here rather than written into a config file so that a commit works in a
/// freshly initialised repository with no ambient identity.
const BASE_ARGS: &[&str] = &[
    "-c",
    "init.defaultBranch=main",
    "-c",
    "user.name=repo-viewer tests",
    "-c",
    "user.email=tests@example.invalid",
    "-c",
    "commit.gpgsign=false",
    // Required for `submodule add` against a local path; see the module docs.
    "-c",
    "protocol.file.allow=always",
];

/// The fixture tree, deleted when this is dropped.
pub struct Fixture {
    /// Held for its `Drop`. The directory goes away with it.
    _dir: TempDir,
    root: PathBuf,
}

impl Fixture {
    /// The tree's root, canonicalised.
    ///
    /// Canonicalised because the temporary directory usually sits behind a link or a short name,
    /// and discovery reports canonical paths — comparing against `TempDir::path` directly fails on
    /// Windows for reasons that have nothing to do with the code under test.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A path inside the tree, from `/`-separated components.
    pub fn path(&self, relative: &str) -> PathBuf {
        let mut path = self.root.clone();
        for part in relative.split('/') {
            path.push(part);
        }
        path
    }
}

/// Build the tree.
///
/// ```text
/// root/
///   node_modules/pkg/   a repository that must never be found — node_modules is pruned
///   notrepo/            an ordinary directory
///   plain/              a normal repository, with one commit
///     nested/           a normal repository — found only with descend_into_repos
///   parent/             a normal repository owning a submodule
///     sub/              the submodule; its .git is a file
///   wt/                 a linked worktree of plain/; its .git is a file too
///   mirror.git/         a bare repository — no .git at all
/// ```
///
/// `node_modules` sits at the root rather than inside `plain/` on purpose. Inside a repository the
/// walk stops before ever reaching it, so the prune predicate would never run and
/// `dirs_pruned` would be `0` whether or not pruning worked.
pub fn build() -> Fixture {
    let dir = TempDir::new().expect("create temp dir");
    let root = dunce::canonicalize(dir.path()).expect("canonicalise temp dir");

    for relative in ["node_modules/pkg", "notrepo", "plain/nested", "parent"] {
        let mut path = root.clone();
        for part in relative.split('/') {
            path.push(part);
        }
        std::fs::create_dir_all(&path).expect("create fixture directory");
    }

    // Repositories that only need to exist.
    for relative in ["node_modules/pkg", "plain/nested"] {
        git(&root.join(relative), &["init"]);
    }

    // `plain` needs a commit: it is the submodule source, and `worktree add` cannot branch from an
    // unborn HEAD.
    let plain = root.join("plain");
    git(&plain, &["init"]);
    commit(&plain, "a.txt");

    let parent = root.join("parent");
    git(&parent, &["init"]);
    commit(&parent, "b.txt");

    // Git wants a forward-slashed path here even on Windows.
    let source = plain.to_string_lossy().replace('\\', "/");
    git(&parent, &["submodule", "add", &source, "sub"]);

    git(&plain, &["worktree", "add", "../wt"]);
    git(&root, &["init", "--bare", "mirror.git"]);

    Fixture { _dir: dir, root }
}

/// Write a file and commit it.
fn commit(repo: &Path, file: &str) {
    std::fs::write(repo.join(file), "fixture\n").expect("write fixture file");
    git(repo, &["add", file]);
    git(repo, &["commit", "-m", "fixture"]);
}

/// Run `git` in `cwd`, panicking with both streams if it fails.
///
/// A fixture that half-built produces a test failure pointing at discovery rather than at the
/// setup, so this is deliberately loud.
fn git(cwd: &Path, args: &[&str]) {
    let missing_config = cwd.join("no-such-gitconfig");
    let output = Command::new("git")
        .current_dir(cwd)
        .args(BASE_ARGS)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &missing_config)
        .env("GIT_CONFIG_SYSTEM", &missing_config)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git — the fixtures need it on PATH");

    assert!(
        output.status.success(),
        "git {args:?} in {} failed with {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        cwd.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
