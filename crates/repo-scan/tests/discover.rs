//! Discovery against a real tree.
//!
//! The fixture is built by `git` at test time (see `support::fixtures`) because every case that
//! matters here needs genuine Git metadata: a `.git` that is a file, a worktree sharing an object
//! store, a repository with no `.git` at all.

mod support;

use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use repo_scan::{DiscoveredRepo, RepoKind, ScanOpts, discover_roots};

use support::fixtures;

/// The deliverable: one walk that finds a linked worktree, a submodule, and a bare repository, and
/// labels each correctly.
///
/// `descend_into_repos` is on because the submodule lives inside `parent/`, and the default walk
/// stops at the first `.git` — which the next test pins.
#[test]
fn finds_a_worktree_a_submodule_and_a_bare_repo() {
    let fixture = fixtures::build();
    let opts = ScanOpts {
        descend_into_repos: true,
        ..ScanOpts::default()
    };

    let (repos, summary) = discover_roots(
        &[fixture.root().to_path_buf()],
        &opts,
        &AtomicBool::new(false),
    );

    assert_eq!(
        kind_of(&repos, &fixture.path("wt")),
        Some(RepoKind::LinkedWorktree)
    );
    assert_eq!(
        kind_of(&repos, &fixture.path("parent/sub")),
        Some(RepoKind::Submodule)
    );
    assert_eq!(
        kind_of(&repos, &fixture.path("mirror.git")),
        Some(RepoKind::Bare)
    );
    assert_eq!(
        kind_of(&repos, &fixture.path("plain")),
        Some(RepoKind::Normal)
    );
    assert_eq!(
        kind_of(&repos, &fixture.path("parent")),
        Some(RepoKind::Normal)
    );
    assert_eq!(
        kind_of(&repos, &fixture.path("plain/nested")),
        Some(RepoKind::Normal)
    );

    assert_eq!(summary.repos_found, 6);
    assert!(
        summary.errors.is_empty(),
        "unexpected errors: {:?}",
        summary.errors
    );
}

/// By default the walk records a repository and stops descending, so a nested checkout and a
/// submodule are both invisible. The independent repositories are still found.
#[test]
fn stops_at_the_first_git_by_default() {
    let fixture = fixtures::build();

    let (repos, summary) = discover_roots(
        &[fixture.root().to_path_buf()],
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );

    assert_eq!(kind_of(&repos, &fixture.path("plain/nested")), None);
    assert_eq!(kind_of(&repos, &fixture.path("parent/sub")), None);
    assert_eq!(
        kind_of(&repos, &fixture.path("wt")),
        Some(RepoKind::LinkedWorktree)
    );
    assert_eq!(
        kind_of(&repos, &fixture.path("mirror.git")),
        Some(RepoKind::Bare)
    );
    assert_eq!(summary.repos_found, 4);
}

/// A submodule is still classified as one when the walk reaches it — here by making it the root.
/// Filling `RepoStatus.submodules` from the parent's config is a Tier 0 concern, not this one.
#[test]
fn a_submodule_reached_directly_is_classified_as_one() {
    let fixture = fixtures::build();
    let sub = fixture.path("parent/sub");

    let (repos, _) = discover_roots(
        std::slice::from_ref(&sub),
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );

    assert_eq!(kind_of(&repos, &sub), Some(RepoKind::Submodule));
}

/// Pruning drops the directory and says so, because a silent name-based omission is the failure
/// mode the count exists to prevent.
#[test]
fn prunes_generated_directories_and_reports_the_count() {
    let fixture = fixtures::build();

    let (repos, summary) = discover_roots(
        &[fixture.root().to_path_buf()],
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );

    assert_eq!(kind_of(&repos, &fixture.path("node_modules/pkg")), None);
    assert_eq!(summary.dirs_pruned, 1);
}

/// The Git directory is resolved during discovery so no later phase has to re-resolve it. A linked
/// worktree points into the main repository's private area; a bare repository *is* its Git
/// directory.
#[test]
fn resolves_the_git_directory_for_each_kind() {
    let fixture = fixtures::build();

    let (repos, _) = discover_roots(
        &[fixture.root().to_path_buf()],
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );

    let worktree = row(&repos, &fixture.path("wt"));
    assert_eq!(worktree.name, "wt");
    assert_eq!(worktree.parent, fixture.root());
    assert_eq!(worktree.git_dir, fixture.path("plain/.git/worktrees/wt"));

    let bare = row(&repos, &fixture.path("mirror.git"));
    assert_eq!(bare.git_dir, bare.path);

    let plain = row(&repos, &fixture.path("plain"));
    assert_eq!(plain.git_dir, fixture.path("plain/.git"));
}

/// Overlapping roots must not produce the same repository twice. Deduplication is on the
/// canonicalised path, which is also what makes a case-differing root collapse.
#[test]
fn deduplicates_overlapping_roots() {
    let fixture = fixtures::build();
    let roots = vec![fixture.root().to_path_buf(), fixture.path("plain")];

    let (repos, _) = discover_roots(&roots, &ScanOpts::default(), &AtomicBool::new(false));

    let plain = fixture.path("plain");
    assert_eq!(repos.iter().filter(|repo| repo.path == plain).count(), 1);
}

/// One bad root must not cost the user the other roots' repositories.
#[test]
fn an_unreadable_root_is_recorded_rather_than_fatal() {
    let fixture = fixtures::build();
    let roots = vec![fixture.path("does-not-exist"), fixture.root().to_path_buf()];

    let (repos, summary) = discover_roots(&roots, &ScanOpts::default(), &AtomicBool::new(false));

    assert_eq!(summary.errors.len(), 1);
    assert_eq!(summary.repos_found, 4);
    assert_eq!(
        kind_of(&repos, &fixture.path("plain")),
        Some(RepoKind::Normal)
    );
}

/// `max_depth` is measured from each root, so a depth of 0 visits the root and nothing below it.
/// The fixture root is not itself a repository, so the walk finds none.
#[test]
fn max_depth_limits_the_descent() {
    let fixture = fixtures::build();
    let opts = ScanOpts {
        max_depth: Some(0),
        ..ScanOpts::default()
    };

    let (repos, _) = discover_roots(
        &[fixture.root().to_path_buf()],
        &opts,
        &AtomicBool::new(false),
    );

    assert!(
        repos.is_empty(),
        "depth 0 should see only the root itself: {repos:?}"
    );
}

/// The kind recorded for `path`, or `None` when it was not found.
fn kind_of(repos: &[DiscoveredRepo], path: &Path) -> Option<RepoKind> {
    repos
        .iter()
        .find(|repo| repo.path == path)
        .map(|repo| repo.kind)
}

/// The row for `path`, or a failure naming what was actually found.
fn row<'a>(repos: &'a [DiscoveredRepo], path: &Path) -> &'a DiscoveredRepo {
    repos
        .iter()
        .find(|repo| repo.path == path)
        .unwrap_or_else(|| {
            let found: Vec<&PathBuf> = repos.iter().map(|repo| &repo.path).collect();
            panic!("no row for {}; found {found:?}", path.display())
        })
}

/// §5.2's case-sensitivity rule: canonicalise for deduplication, report the on-disk form.
///
/// Windows-only, because this asserts that two spellings of one path collapse — on a
/// case-sensitive filesystem the upper-cased spelling is a different path that does not exist.
#[cfg(windows)]
#[test]
fn a_case_differing_root_collapses_onto_the_on_disk_form() {
    let fixture = fixtures::build();
    let shouted = fixture.root().to_string_lossy().to_uppercase();
    let roots = vec![fixture.root().to_path_buf(), PathBuf::from(&shouted)];

    let (repos, summary) = discover_roots(&roots, &ScanOpts::default(), &AtomicBool::new(false));

    assert!(
        summary.errors.is_empty(),
        "unexpected errors: {:?}",
        summary.errors
    );
    assert_eq!(summary.repos_found, 4);
    // The reported path is the on-disk spelling, not the one that was asked for.
    let plain = row(&repos, &fixture.path("plain"));
    assert!(!plain.path.to_string_lossy().starts_with(&shouted));
}

/// A pre-set flag stops the walk before it reports anything.
///
/// Mirrors `tier0.rs`'s `cancellation_stops_the_pass`. The uncancelled control call is not
/// decoration: without it this test also passes when the fixture tree is empty or the root is
/// wrong, which is the failure mode a cancellation test is most likely to have.
#[test]
fn cancellation_stops_the_walk() {
    let fixture = fixtures::build();

    let (found, _) = discover_roots(
        &[fixture.root().to_path_buf()],
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );
    assert!(
        !found.is_empty(),
        "the fixture tree should hold repositories"
    );

    // Pre-set so the outcome does not depend on winning a race with the walk.
    let cancelled = AtomicBool::new(true);
    let (repos, summary) = discover_roots(
        &[fixture.root().to_path_buf()],
        &ScanOpts::default(),
        &cancelled,
    );

    assert!(repos.is_empty());
    assert_eq!(summary.repos_found, 0);
    assert!(
        summary.errors.is_empty(),
        "cancelling is not a failure: {:?}",
        summary.errors
    );
}
