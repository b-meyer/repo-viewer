//! Builds the fixture trees at test time.
//!
//! Two of them, deliberately separate. [`build`] is the discovery tree, whose exact `repos_found`
//! and `dirs_visited` counts `discover.rs` asserts — adding a repository to it would make every
//! one of those counts a maintenance tax on tests that do not care. [`status_tree`] is the Tier 0
//! tree: upstream topologies, parked operations, and the head shapes.
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
    sync::OnceLock,
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

/// Run `git` in `cwd` and hand back the finished process.
///
/// The one place `BASE_ARGS` and the environment overrides are applied, so every caller — the
/// builders, the oracle, and the deliberately-failing operations — gets the same isolation.
fn run(cwd: &Path, args: &[&str]) -> std::process::Output {
    let missing_config = cwd.join("no-such-gitconfig");
    Command::new("git")
        .current_dir(cwd)
        .args(BASE_ARGS)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &missing_config)
        .env("GIT_CONFIG_SYSTEM", &missing_config)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git — the fixtures need it on PATH")
}

/// Run `git` in `cwd`, panicking with both streams if it fails.
///
/// A fixture that half-built produces a test failure pointing at discovery rather than at the
/// setup, so this is deliberately loud.
fn git(cwd: &Path, args: &[&str]) {
    let output = run(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} in {} failed with {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        cwd.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Run `git` in `cwd` and ignore a non-zero exit.
///
/// For the operations whose *whole point* is to fail: a conflicting revert, cherry-pick, or rebase
/// is what leaves `REVERT_HEAD`, `CHERRY_PICK_HEAD`, or `rebase-merge/` on disk, and those parked
/// states are what the `RepoState` mapping is read from. A clean revert commits and leaves nothing
/// behind, so success here would build the wrong fixture.
fn git_may_fail(cwd: &Path, args: &[&str]) {
    let _ = run(cwd, args);
}

/// Run `git` in `cwd` and return its trimmed stdout, panicking if it fails.
///
/// `pub` because the ahead/behind oracle lives in the test file rather than here: asserting
/// against `git rev-list --left-right --count` is the point of those tests, so the comparison
/// belongs where it can be read beside the expectation.
pub fn git_out(cwd: &Path, args: &[&str]) -> String {
    let output = run(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} in {} failed with {}\n--- stderr ---\n{}",
        cwd.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

// -------------------------------------------------------------------------------------------
// The status tree
// -------------------------------------------------------------------------------------------

/// The status fixture tree, built once per test binary.
///
/// Building it costs upwards of sixty `git` invocations, and Windows process creation is the
/// expensive part, so the tree is shared rather than rebuilt per test. The `OnceLock` holds it —
/// and its `TempDir` — for the life of the process, which is exactly the lifetime wanted: the
/// directory goes away when the test binary exits.
///
/// Nothing here touches a repository the developer owns. Every `fetch`, `push`, and `stash` runs
/// inside a `tempfile::TempDir`; the read-only rule protects real repositories, not fixtures.
pub fn status_tree() -> &'static Fixture {
    static TREE: OnceLock<Fixture> = OnceLock::new();
    TREE.get_or_init(build_status)
}

/// Build the tree Tier 0 is read from.
///
/// ```text
/// root/
///   origin.git/     bare, the shared push target for every clone below
///   seed/           authors the upstream-side commits
///   pusher/         a second author, needed for the criss-cross topology
///   synced/         local == upstream                        ahead 0, behind 0
///   ahead/          two local commits                        ahead 2, behind 0
///   behind/         three upstream commits, fetched          ahead 0, behind 3
///   diverged/       both                                     ahead 2, behind 3
///   merged/         upstream merged INTO local               ahead 2, behind 0
///   crisscross/     two merge bases                          both non-zero
///   noupstream/     no remote at all, never fetched          all None
///   gone/           upstream configured, tracking ref gone   name but no counts
///   detached/       detached onto a commit
///   taggedhead/     detached onto an annotated TAG object
///   unborn/         init only, no commits
///   stashed/        two stash entries
///   merging/        MERGE_HEAD parked
///   reverting/      REVERT_HEAD parked
///   cherrypicking/  CHERRY_PICK_HEAD parked
///   bisecting/      BISECT_LOG parked
///   rebasing/       a conflicted rebase parked
///   bare.git/       bare, reads Tier 0 with no worktree
///   wt/             a linked worktree of behind/, which has a FETCH_HEAD
/// ```
///
/// Clone order is load-bearing. Every clone starts at the same upstream tip, so each topology is
/// built by moving one side afterwards — which means `behind` must be cloned *before* `seed`
/// pushes the commits it is meant to be behind by.
fn build_status() -> Fixture {
    let dir = TempDir::new().expect("create temp dir");
    let root = dunce::canonicalize(dir.path()).expect("canonicalise temp dir");

    // The shared upstream, seeded with two commits so a clone has something to detach to.
    git(&root, &["init", "--bare", "origin.git"]);
    let origin = local_url(&root.join("origin.git"));

    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).expect("create seed");
    git(&seed, &["init"]);
    commit(&seed, "a.txt");
    git(&seed, &["remote", "add", "origin", &origin]);
    commit(&seed, "b.txt");
    git(&seed, &["push", "-u", "origin", "main"]);

    // Every clone below starts at that tip. Cloning a local path needs
    // `protocol.file.allow=always`, which BASE_ARGS already carries.
    for name in [
        "synced",
        "ahead",
        "behind",
        "diverged",
        "merged",
        "crisscross",
        "gone",
        "detached",
        "taggedhead",
    ] {
        git(&root, &["clone", &origin, name]);
    }

    // ahead: local moves, upstream does not.
    let ahead = root.join("ahead");
    commit(&ahead, "local-1.txt");
    commit(&ahead, "local-2.txt");

    // behind: upstream moves, local only fetches. That fetch is also what writes FETCH_HEAD, so
    // `behind` is the fixture that has a `last_fetched_ms`.
    for file in ["up-1.txt", "up-2.txt", "up-3.txt"] {
        commit(&seed, file);
    }
    git(&seed, &["push", "origin", "main"]);
    git(&root.join("behind"), &["fetch"]);

    // diverged: both sides moved. Upstream is already three past the clone point.
    let diverged = root.join("diverged");
    commit(&diverged, "local-1.txt");
    commit(&diverged, "local-2.txt");
    git(&diverged, &["fetch"]);

    // merged: upstream merged into local, so nothing upstream is unreachable from local and
    // `behind` must be 0. This is the topology `with_boundary` overcounts, because it stops at
    // the upstream tip without hiding its ancestors.
    let merged = root.join("merged");
    commit(&merged, "local-1.txt");
    git(&merged, &["fetch"]);
    git(&merged, &["merge", "--no-edit", "origin/main"]);

    build_crisscross(&root, &origin);

    // noupstream: no remote at all, and never fetched — the `last_fetched_ms == None` fixture.
    let noupstream = root.join("noupstream");
    std::fs::create_dir_all(&noupstream).expect("create noupstream");
    git(&noupstream, &["init"]);
    commit(&noupstream, "a.txt");

    // gone: the upstream *config* survives, the tracking ref does not — a branch whose remote
    // branch was deleted, which must read as "upstream known, counts unknown".
    git(
        &root.join("gone"),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );

    git(&root.join("detached"), &["checkout", "--detach", "HEAD~1"]);
    build_tagged_head(&root.join("taggedhead"));

    let unborn = root.join("unborn");
    std::fs::create_dir_all(&unborn).expect("create unborn");
    git(&unborn, &["init"]);

    build_stashed(&root.join("stashed"));
    build_parked_states(&root);

    git(&root, &["init", "--bare", "bare.git"]);

    // A linked worktree of `behind`, which already fetched and so has a FETCH_HEAD in its common
    // directory and none in the worktree's own private Git directory.
    //
    // Deliberately not of `ahead`: fetching there to produce a FETCH_HEAD would also advance its
    // `origin/main` past every commit pushed since it was cloned, and a fixture called `ahead`
    // that is quietly six behind teaches the reader the wrong thing.
    git(&root.join("behind"), &["worktree", "add", "../wt"]);

    Fixture { _dir: dir, root }
}

/// Build a history with **two** merge bases.
///
/// Each side merges the other's first commit, which is what produces a criss-cross rather than a
/// single base. It needs a second author on the upstream side, because one clone cannot both
/// advance `origin/main` and stay behind it.
fn build_crisscross(root: &Path, origin: &str) {
    let local = root.join("crisscross");
    let pusher = root.join("pusher");

    // L1 lands on a side branch so `origin/main` stays free for the other author.
    commit(&local, "local-1.txt");
    git(&local, &["push", "origin", "main:refs/heads/side"]);

    // A filename no earlier fixture used. `commit` writes fixed contents, so re-using a name that
    // is already in the clone leaves the worktree clean and `git commit` fails with nothing to do.
    git(root, &["clone", origin, "pusher"]);
    commit(&pusher, "pusher-1.txt");
    git(&pusher, &["push", "origin", "main"]);

    // Local merges the other side's commit.
    git(&local, &["fetch"]);
    git(&local, &["merge", "--no-edit", "origin/main"]);

    // The other side merges local's, so each tip is a merge whose parents include the other's
    // base. That is what leaves two merge bases between them.
    git(&pusher, &["fetch"]);
    git(&pusher, &["merge", "--no-edit", "origin/side"]);
    git(&pusher, &["push", "origin", "main"]);

    // One further commit locally, so neither count comes out as 1 by accident.
    git(&local, &["fetch"]);
    commit(&local, "local-2.txt");
}

/// Detach HEAD onto an annotated **tag object**, not the commit it points at.
///
/// `git checkout <annotated-tag>` writes the *commit* id, so git will not produce this state on
/// its own — the tag id goes into `.git/HEAD` directly. It is the one state that separates
/// `Head::try_peel_to_id()` from `Head::id()`: the latter reads a `peeled` field that only a
/// `packed-refs` `^` line ever populates, so on a loose HEAD it hands back the tag.
fn build_tagged_head(repo: &Path) {
    git(repo, &["tag", "-a", "v1", "-m", "annotated"]);
    let tag_id = git_out(repo, &["rev-parse", "v1"]);
    std::fs::write(repo.join(".git").join("HEAD"), format!("{tag_id}\n"))
        .expect("write detached HEAD");
}

/// Two stash entries, which is two reflog lines on `refs/stash`.
fn build_stashed(repo: &Path) {
    std::fs::create_dir_all(repo).expect("create stashed");
    git(repo, &["init"]);
    commit(repo, "a.txt");
    for contents in ["one\n", "two\n"] {
        std::fs::write(repo.join("a.txt"), contents).expect("dirty the worktree");
        git(repo, &["stash", "push", "-m", "fixture"]);
    }
}

/// One repository per parked operation, so every non-`Clean` state is reachable.
///
/// The conflicting cases are parked by real conflicting operations rather than by writing marker
/// files, because the markers are gix's contract to read while the mapping is ours to test — a
/// hand-written `REVERT_HEAD` would keep passing even if git stopped writing one.
fn build_parked_states(root: &Path) {
    // merging: a clean, uncommitted merge. Exits 0 and leaves MERGE_HEAD.
    let merging = root.join("merging");
    std::fs::create_dir_all(&merging).expect("create merging");
    git(&merging, &["init"]);
    commit(&merging, "base.txt");
    git(&merging, &["checkout", "-b", "other"]);
    commit(&merging, "other.txt");
    git(&merging, &["checkout", "main"]);
    commit(&merging, "main.txt");
    git(&merging, &["merge", "--no-commit", "--no-ff", "other"]);

    // The rest need a one-line conflict to stop mid-operation.
    for (name, parked) in [
        ("reverting", Parked::Revert),
        ("cherrypicking", Parked::CherryPick),
        ("rebasing", Parked::Rebase),
    ] {
        let repo = root.join(name);
        std::fs::create_dir_all(&repo).expect("create parked repo");
        git(&repo, &["init"]);
        conflicting_history(&repo);
        match parked {
            // Reverting the middle commit conflicts, because the tip touched the same line.
            Parked::Revert => git_may_fail(&repo, &["revert", "HEAD~1"]),
            Parked::CherryPick => git_may_fail(&repo, &["cherry-pick", "side"]),
            Parked::Rebase => {
                git(&repo, &["checkout", "side"]);
                git_may_fail(&repo, &["rebase", "main"]);
            }
        }
    }

    // bisecting: a started bisect writes BISECT_LOG.
    let bisecting = root.join("bisecting");
    std::fs::create_dir_all(&bisecting).expect("create bisecting");
    git(&bisecting, &["init"]);
    commit(&bisecting, "a.txt");
    let first = git_out(&bisecting, &["rev-parse", "HEAD"]);
    commit(&bisecting, "b.txt");
    commit(&bisecting, "c.txt");
    git(&bisecting, &["bisect", "start"]);
    git(&bisecting, &["bisect", "bad"]);
    git_may_fail(&bisecting, &["bisect", "good", &first]);
}

/// Which operation to park a repository in.
enum Parked {
    /// A conflicting `git revert`.
    Revert,
    /// A conflicting `git cherry-pick`.
    CherryPick,
    /// A conflicting `git rebase`.
    Rebase,
}

/// Two branches that edit the same line, so any replay between them conflicts.
fn conflicting_history(repo: &Path) {
    write_commit(repo, "c.txt", "base\n", "base");
    git(repo, &["checkout", "-b", "side"]);
    write_commit(repo, "c.txt", "side\n", "side");
    git(repo, &["checkout", "main"]);
    write_commit(repo, "c.txt", "main-one\n", "main one");
    write_commit(repo, "c.txt", "main-two\n", "main two");
}

/// Write `contents` to `file` and commit it with `message`.
fn write_commit(repo: &Path, file: &str, contents: &str, message: &str) {
    std::fs::write(repo.join(file), contents).expect("write fixture file");
    git(repo, &["add", file]);
    git(repo, &["commit", "-m", message]);
}

/// A path as git wants it in a URL position: forward slashes, even on Windows.
fn local_url(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
