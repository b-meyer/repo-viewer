//! What to watch for one repository.
//!
//! Three places inside the Git directory, and never the worktree. Recursively watching worktrees
//! means recursively watching `node_modules`, which is how a tool burns a core and blows past
//! inotify's per-user limit. A worktree edit is picked up by the poll, by refresh-on-focus, or by
//! the `index` write that follows any `git add`.
//!
//! Pure: this module builds paths and asks the filesystem whether they exist. It registers nothing,
//! which is what lets the §7.2 set be asserted without a live watcher anywhere near the test.

use std::path::{Path, PathBuf};

use notify::RecursiveMode;

use crate::model::DiscoveredRepo;

/// One registration: a path and how deep to watch it.
pub type Watch = (PathBuf, RecursiveMode);

/// The paths to register for `repo`, in **registration order** — shallowest first.
///
/// Order is load-bearing on the way back out rather than on the way in: `unwatch` drops the
/// debouncer's record of every root beneath the path it is given, so a caller removing a
/// repository has to work through this list in reverse. See [`super::RepoWatcher::sync`].
///
/// The set, and why each entry earns its place:
///
/// - **the Git directory root, non-recursive** — `HEAD`, `index`, `packed-refs`, `FETCH_HEAD`,
///   `ORIG_HEAD`, `MERGE_HEAD`, and the `index.lock` burst that every write produces. Non-recursive
///   because the interesting children are files sitting directly in it, and because `objects/` is
///   underneath and is the one directory in a repository that can be enormous.
/// - **`refs/`, recursive** — a non-recursive watch sees only direct children, so it would miss
///   every slash-named branch (`refs/heads/feature/x`) and every remote-tracking update
///   (`refs/remotes/origin/main`), which is exactly the ahead/behind signal. The refs tree is a few
///   dozen small files, so recursion here costs nothing.
/// - **`logs/HEAD`** — appended on every commit, checkout and reset, whatever the branch is called.
///
/// A linked worktree contributes those from **both** directories, and both halves earn it. Its
/// private `git_dir` holds its own `HEAD`, `index` and `logs/HEAD` — and its own `refs/`, which is
/// not decoration: per-worktree refs (`refs/bisect/*`, `refs/worktree/*`) live there rather than in
/// the common directory, so a bisect running in this worktree is visible nowhere else. The branches
/// and remote-tracking refs it reads, and its `FETCH_HEAD`, live in the shared common directory.
/// Every other kind has `git_dir == common_dir` and so contributes one set.
///
/// **Filtered by existence**, which is not defensive tidiness: `notify`'s `watch()` fails with
/// `PathNotFound`, and `logs/HEAD` does not exist until a repository's first commit. An unfiltered
/// set would report a registration failure for every freshly `init`ed repository in the tree.
pub fn watch_set(repo: &DiscoveredRepo) -> Vec<Watch> {
    let mut watches = Vec::with_capacity(5);

    push_dir(&mut watches, &repo.git_dir, RecursiveMode::NonRecursive);
    push_dir(
        &mut watches,
        &repo.git_dir.join("refs"),
        RecursiveMode::Recursive,
    );
    push_file(&mut watches, &repo.git_dir.join("logs").join("HEAD"));

    // A linked worktree, and only then. `common_dir` is resolved by discovery, so this comparison
    // is the whole of the "is this a linked worktree" question at registration time — no need to
    // consult `kind`, which would leave the two able to disagree.
    if repo.common_dir != repo.git_dir {
        push_dir(&mut watches, &repo.common_dir, RecursiveMode::NonRecursive);
        push_dir(
            &mut watches,
            &repo.common_dir.join("refs"),
            RecursiveMode::Recursive,
        );
        push_file(&mut watches, &repo.common_dir.join("logs").join("HEAD"));
    }

    watches
}

/// Add `path` if it is a directory.
fn push_dir(watches: &mut Vec<Watch>, path: &Path, mode: RecursiveMode) {
    if path.is_dir() {
        watches.push((path.to_path_buf(), mode));
    }
}

/// Add `path` if it is a file. Always non-recursive — a file has nothing below it.
fn push_file(watches: &mut Vec<Watch>, path: &Path) {
    if path.is_file() {
        watches.push((path.to_path_buf(), RecursiveMode::NonRecursive));
    }
}
