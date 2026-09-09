//! Tier 1 against a real tree: the dirty flag and the conflicted count.
//!
//! Three of these tests exist because three different plausible implementations are wrong in three
//! different ways, and a single "dirty repository" fixture would let two of them pass. Each names
//! the mistake it catches.

mod support;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use repo_scan::{DiscoveredRepo, RepoKind, ScanOpts, Tier1, discover_roots, read_tier1_all_with};

use support::fixtures;

/// Every Tier 1 result for the status tree, keyed by path.
fn read_tree() -> (
    HashMap<PathBuf, Tier1>,
    repo_scan::Tier1Summary,
    Vec<DiscoveredRepo>,
) {
    let tree = fixtures::status_tree();
    let (found, _) = discover_roots(
        &[tree.root().to_path_buf()],
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );

    let rows = std::sync::Mutex::new(HashMap::new());
    let summary = read_tier1_all_with(&found, &Arc::new(AtomicBool::new(false)), |status| {
        rows.lock()
            .expect("no test holds this across a panic")
            .insert(status.path.clone(), status);
    });

    (
        rows.into_inner().expect("lock is not poisoned"),
        summary,
        found,
    )
}

/// The result for a named fixture repository.
fn row<'a>(rows: &'a HashMap<PathBuf, Tier1>, tree_path: &Path) -> &'a Tier1 {
    rows.get(tree_path).unwrap_or_else(|| {
        panic!(
            "no Tier 1 row for {}; got {:?}",
            tree_path.display(),
            rows.keys().collect::<Vec<_>>()
        )
    })
}

/// **The `is_dirty()` trap.** `gix::Repository::is_dirty()` documents that untracked files do not
/// affect it, and it disables the directory walk internally — so an implementation built on it
/// reports this repository clean.
#[test]
fn a_repo_whose_only_change_is_an_untracked_file_is_dirty() {
    let tree = fixtures::status_tree();
    let (rows, _, _) = read_tree();

    assert!(
        row(&rows, &tree.path("untracked")).dirty,
        "an untracked file is a change; is_dirty() alone would miss it"
    );
}

/// **The `into_index_worktree_iter()` trap.** That iterator sets `head_tree = None`, so it compares
/// only the index against the worktree. Here the worktree *matches* the index and the change is
/// between HEAD's tree and the index, so only the tree-against-index half sees it.
#[test]
fn a_repo_whose_only_change_is_staged_is_dirty() {
    let tree = fixtures::status_tree();
    let (rows, _, _) = read_tree();

    assert!(
        row(&rows, &tree.path("stagedonly")).dirty,
        "a staged change is uncommitted work; into_index_worktree_iter() would miss it"
    );
}

/// The case every implementation gets right. Here so that a flag which is somehow always `true`
/// cannot pass by agreeing with the two tests above.
#[test]
fn a_repo_with_an_unstaged_modification_is_dirty() {
    let tree = fixtures::status_tree();
    let (rows, _, _) = read_tree();

    assert!(row(&rows, &tree.path("unstaged")).dirty);
}

/// The other half of that guard: a pristine clone must read clean.
#[test]
fn an_untouched_clone_is_clean() {
    let tree = fixtures::status_tree();
    let (rows, _, _) = read_tree();

    let synced = row(&rows, &tree.path("synced"));
    assert!(!synced.dirty, "a pristine clone has nothing uncommitted");
    assert_eq!(synced.conflicted, 0);
    assert_eq!(synced.error, None);
}

/// **The stage-counting trap.** A modify/modify merge leaves stages 1, 2 and 3 for each path, so
/// this repository has six conflicted index entries across two files. Counting entries gives 6;
/// counting distinct paths gives 2, which is what `git status` reports.
#[test]
fn a_conflicted_merge_counts_files_not_index_entries() {
    let tree = fixtures::status_tree();
    let (rows, _, _) = read_tree();
    let conflicted = row(&rows, &tree.path("conflicted"));

    assert_eq!(
        conflicted.conflicted, 2,
        "two files conflict; six index entries describe them"
    );
    assert!(conflicted.dirty, "a parked conflict is uncommitted work");
}

/// A repository with no conflict reports zero rather than leaving the count unknown: the read
/// succeeded, and zero is the answer.
#[test]
fn an_unconflicted_repo_reports_zero_conflicts() {
    let tree = fixtures::status_tree();
    let (rows, _, _) = read_tree();

    assert_eq!(row(&rows, &tree.path("untracked")).conflicted, 0);
}

/// A bare repository has no worktree, so Tier 1 does not apply to it at all.
///
/// It is skipped rather than reported clean. Reporting `dirty: false` would be a claim about a
/// worktree that does not exist, and it is what lets the row keep saying so.
#[test]
fn a_bare_repository_is_skipped_rather_than_reported_clean() {
    let tree = fixtures::status_tree();
    let (rows, summary, found) = read_tree();

    assert!(
        !rows.contains_key(&tree.path("bare.git")),
        "a bare repository must produce no Tier 1 row"
    );
    let bare = found
        .iter()
        .filter(|repo| repo.kind == RepoKind::Bare)
        .count();
    assert!(bare > 0, "the fixture tree should hold a bare repository");
    assert_eq!(summary.bare_skipped as usize, bare);
}

/// A linked worktree has its own index and worktree, so Tier 1 reads it like any other repository.
#[test]
fn a_linked_worktree_reads_tier_1() {
    let tree = fixtures::status_tree();
    let (rows, _, _) = read_tree();

    assert!(rows.contains_key(&tree.path("wt")));
}

/// Every repository with a worktree produced a row, and none of them failed.
#[test]
fn every_worktree_repository_is_read() {
    let (rows, summary, found) = read_tree();

    let with_worktree = found
        .iter()
        .filter(|repo| repo.kind != RepoKind::Bare)
        .count();

    assert_eq!(summary.repos_read as usize, rows.len());
    assert_eq!(rows.len(), with_worktree);
    assert!(
        summary.errors.is_empty(),
        "unexpected Tier 1 failures: {:?}",
        summary.errors
    );
}

/// Cancelling stops the pass and is not an error.
#[test]
fn cancellation_stops_the_pass() {
    let tree = fixtures::status_tree();
    let (found, _) = discover_roots(
        &[tree.root().to_path_buf()],
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );
    assert!(
        !found.is_empty(),
        "the fixture tree should hold repositories"
    );

    // Pre-set so the outcome does not depend on winning a race with the fan-out.
    let cancelled = Arc::new(AtomicBool::new(true));
    let seen = std::sync::Mutex::new(Vec::new());
    let summary = read_tier1_all_with(&found, &cancelled, |status| {
        seen.lock().expect("not poisoned").push(status);
    });

    assert!(seen.into_inner().expect("not poisoned").is_empty());
    assert_eq!(summary.repos_read, 0);
    assert!(
        summary.errors.is_empty(),
        "cancelling is not a failure: {:?}",
        summary.errors
    );
}

/// **A dirty repository's early exit must not interrupt the repositories beside it.**
///
/// `gix` treats the flag handed to `should_interrupt_owned` as one it may write: dropping a status
/// iterator sets it to stop the worker threads, restoring it only afterwards. Sharing one flag
/// across the rayon fan-out therefore let a dirty repository — which exits after the first item —
/// abort whichever neighbours were mid-walk, and they came back as `Interrupted` errors with
/// nothing having asked them to stop.
///
/// The pass is run repeatedly because the window is small and the failure is a race. It reproduced
/// within a handful of iterations on a tree of this shape, where dirty and clean repositories are
/// read concurrently; the fix makes it impossible rather than unlikely.
#[test]
fn one_repositorys_early_exit_does_not_interrupt_the_others() {
    for attempt in 0..8 {
        let (rows, summary, found) = read_tree();

        assert!(
            summary.errors.is_empty(),
            "attempt {attempt}: nothing cancelled this pass, so nothing may report a failure: {:?}",
            summary.errors
        );
        assert_eq!(
            rows.len() + summary.bare_skipped as usize,
            found.len(),
            "attempt {attempt}: every repository produced a result or was skipped as bare"
        );
    }
}

/// The dirty flag agrees with `git status --porcelain` on every fixture repository.
///
/// The oracle matters here for the same reason it does for ahead/behind: a literal encodes what the
/// author believed the fixture contained, and git's answer cannot drift from git's behaviour. The
/// literals above are kept as well, so an implementation returning `true` everywhere cannot pass by
/// agreeing with an oracle that also reads dirty.
#[test]
fn the_dirty_flag_matches_git_status() {
    let (rows, _, found) = read_tree();

    for repo in &found {
        if repo.kind == RepoKind::Bare {
            continue;
        }
        // `--porcelain` includes untracked files by default, which is the definition Tier 1 uses.
        let porcelain = fixtures::git_out(&repo.path, &["status", "--porcelain"]);
        let expected = !porcelain.trim().is_empty();

        assert_eq!(
            row(&rows, &repo.path).dirty,
            expected,
            "dirty flag disagrees with `git status --porcelain` for {}:\n{porcelain}",
            repo.path.display()
        );
    }
}
