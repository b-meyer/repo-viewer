//! Tier 0 reads, against a `git`-built fixture tree.
//!
//! The ahead/behind tests assert against `git rev-list --left-right --count` rather than against
//! hand-counted numbers. That is the point of them: a hand-counted expectation encodes whatever
//! the author believed the topology was, and the topologies that matter here — a merge from
//! upstream, a criss-cross — are exactly the ones where that belief is unreliable. Git's own
//! answer cannot drift from git's own behaviour.
//!
//! One test deliberately provokes a caught panic, so a scary-looking panic message and backtrace
//! during a passing run is expected. `catch_unwind` still runs the process panic hook, and a
//! library has no business installing one.

mod support;

use std::{collections::HashMap, sync::atomic::AtomicBool};

use repo_scan::{
    AHEAD_BEHIND_CAP, DiscoveredRepo, Head, RepoKind, RepoState, RepoStatus, ScanOpts,
    ahead_behind, discover_roots, read_tier0, read_tier0_all,
};
use support::fixtures;

/// Read every repository in the status tree, keyed by directory name.
///
/// Goes through discovery rather than hand-building `DiscoveredRepo` values, so the tests exercise
/// the same `git_dir` resolution the scan path uses — which is the whole reason Tier 0 does not
/// re-resolve it.
fn rows() -> HashMap<String, RepoStatus> {
    let tree = fixtures::status_tree();
    let opts = ScanOpts {
        // The tree nests a worktree and several clones one level down; the default is ample, but
        // stating it keeps the fixture independent of that default changing.
        max_depth: Some(4),
        ..ScanOpts::default()
    };
    let (found, summary) = discover_roots(&[tree.root().to_path_buf()], &opts);
    assert!(
        summary.errors.is_empty(),
        "discovery of the fixture tree reported errors: {:?}",
        summary.errors
    );

    let (statuses, tier0) = read_tier0_all(&found, &AtomicBool::new(false));
    assert!(
        tier0.errors.is_empty(),
        "tier 0 could not read some fixtures: {:?}",
        tier0.errors
    );
    assert_eq!(
        tier0.repos_read as usize,
        found.len(),
        "every discovered fixture should produce a row"
    );

    statuses
        .into_iter()
        .map(|status| (status.name.clone(), status))
        .collect()
}

/// The row for `name`, or a failure naming what was actually found.
fn row<'a>(rows: &'a HashMap<String, RepoStatus>, name: &str) -> &'a RepoStatus {
    rows.get(name).unwrap_or_else(|| {
        let mut names: Vec<&str> = rows.keys().map(String::as_str).collect();
        names.sort_unstable();
        panic!("no `{name}` row; the tree produced {names:?}")
    })
}

/// Git's own ahead/behind for a repository, as `(ahead, behind)`.
///
/// `git rev-list --left-right --count HEAD...@{upstream}` prints `<ahead>\t<behind>` — the left
/// side is what HEAD has and upstream does not.
fn oracle(rows: &HashMap<String, RepoStatus>, name: &str) -> (u32, u32) {
    let path = &row(rows, name).path;
    let counts = fixtures::git_out(
        path,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    );
    let mut parts = counts.split_whitespace();
    let ahead = parts
        .next()
        .expect("ahead count")
        .parse()
        .expect("a number");
    let behind = parts
        .next()
        .expect("behind count")
        .parse()
        .expect("a number");
    (ahead, behind)
}

#[test]
fn ahead_behind_matches_git_rev_list() {
    let rows = rows();

    // Every fixture that has both an upstream name and counts must agree with git.
    let mut compared = 0;
    for (name, status) in &rows {
        if status.upstream.is_none() || status.ahead.is_none() {
            continue;
        }
        let (ahead, behind) = oracle(&rows, name);
        assert_eq!(
            (status.ahead, status.behind),
            (Some(ahead), Some(behind)),
            "`{name}` disagrees with git rev-list"
        );
        compared += 1;
    }
    assert!(
        compared >= 6,
        "only {compared} rows were compared against the oracle; the topology fixtures are missing"
    );

    // An implementation returning (0, 0) everywhere would match an oracle that also read 0, so
    // the interesting topologies are pinned to non-trivial values as well.
    assert_eq!(row(&rows, "ahead").ahead, Some(2));
    assert_eq!(row(&rows, "ahead").behind, Some(0));
    assert_eq!(row(&rows, "behind").ahead, Some(0));
    assert_eq!(row(&rows, "behind").behind, Some(3));
    assert_eq!(row(&rows, "diverged").ahead, Some(2));
    assert_eq!(row(&rows, "diverged").behind, Some(3));
}

#[test]
fn a_merge_from_upstream_is_not_overcounted() {
    let rows = rows();
    let merged = row(&rows, "merged");

    // The `with_boundary` regression, pinned by a literal as well as by the oracle. A boundary
    // stops the walk at the upstream tip without hiding its ancestors, so every commit upstream
    // contributed would be counted as local work.
    assert_eq!(merged.behind, Some(0), "upstream is fully merged in");
    assert_eq!(
        merged.ahead,
        Some(2),
        "only the local commit and the merge itself are ahead"
    );
}

#[test]
fn a_criss_cross_merge_is_counted_correctly() {
    let rows = rows();
    let crisscross = row(&rows, "crisscross");

    // A fixture that quietly degenerated into a fast-forward would satisfy the oracle trivially,
    // so assert the shape that makes it worth testing: two merge bases.
    let bases = fixtures::git_out(
        &crisscross.path,
        &["merge-base", "--all", "HEAD", "@{upstream}"],
    );
    assert_eq!(
        bases.lines().count(),
        2,
        "the criss-cross fixture should have two merge bases, got:\n{bases}"
    );

    let (ahead, behind) = oracle(&rows, "crisscross");
    assert_eq!(crisscross.ahead, Some(ahead));
    assert_eq!(crisscross.behind, Some(behind));
    assert!(
        ahead > 0 && behind > 0,
        "both sides should have unique commits, got {ahead}/{behind}"
    );
}

#[test]
fn the_walk_is_capped() {
    let tree = fixtures::status_tree();
    let repo = gix::open(tree.path("ahead")).expect("open the ahead fixture");

    let local = repo.head_id().expect("local tip").detach();
    let upstream = repo
        .find_reference("refs/remotes/origin/main")
        .expect("tracking ref")
        .into_fully_peeled_id()
        .expect("peel the tracking ref")
        .detach();

    // `ahead` is two commits ahead, so a cap of one must report one rather than two. Testing the
    // cap through a thousand-commit fixture would dominate the suite, which is why the cap is a
    // parameter rather than only a constant.
    let capped = ahead_behind(&repo, local, upstream, 1).expect("count with a cap of one");
    assert_eq!(capped, (1, 0), "the walk stops at the cap");

    let uncapped = ahead_behind(&repo, local, upstream, 10).expect("count with room to spare");
    assert_eq!(uncapped, (2, 0), "below the cap the count is exact");

    // The default is what the scan path uses, and it is well past any real row.
    assert_eq!(AHEAD_BEHIND_CAP, 1000);
}

#[test]
fn an_equal_upstream_is_zero_and_not_unknown() {
    let rows = rows();
    let synced = row(&rows, "synced");

    // The fast path, and the other half of the unknown-versus-zero distinction: in sync is a
    // known answer of zero, not an absent one.
    assert_eq!(synced.ahead, Some(0));
    assert_eq!(synced.behind, Some(0));
    assert_eq!(synced.upstream.as_deref(), Some("origin/main"));
}

#[test]
fn no_upstream_leaves_the_counts_unknown() {
    let rows = rows();
    let noupstream = row(&rows, "noupstream");

    // The invariant the whole `Option` discipline exists for: no upstream means unknown, never 0.
    assert_eq!(noupstream.upstream, None);
    assert_eq!(noupstream.ahead, None);
    assert_eq!(noupstream.behind, None);
}

#[test]
fn a_missing_tracking_ref_reports_the_name_but_no_counts() {
    let rows = rows();
    let gone = row(&rows, "gone");

    // Configured upstream, deleted tracking ref. The name is known and worth showing; the counts
    // have nothing to measure against and must not read as zero.
    assert_eq!(gone.upstream.as_deref(), Some("origin/main"));
    assert_eq!(gone.ahead, None);
    assert_eq!(gone.behind, None);
    assert_eq!(
        gone.error, None,
        "a deleted tracking ref is a state, not a failure"
    );
}

#[test]
fn head_shapes_are_reported() {
    let rows = rows();

    assert_eq!(
        row(&rows, "synced").head,
        Head::Branch {
            name: "main".to_string()
        },
        "the short branch name, not refs/heads/main"
    );
    assert_eq!(row(&rows, "unborn").head, Head::Unborn);

    let detached = row(&rows, "detached");
    let expected = fixtures::git_out(&detached.path, &["rev-parse", "HEAD"]);
    assert_eq!(detached.head, Head::Detached { id: expected });
}

#[test]
fn a_detached_tag_checkout_reports_the_commit_not_the_tag() {
    let rows = rows();
    let tagged = row(&rows, "taggedhead");

    let tag = fixtures::git_out(&tagged.path, &["rev-parse", "v1"]);
    let commit = fixtures::git_out(&tagged.path, &["rev-parse", "v1^{commit}"]);
    assert_ne!(tag, commit, "the fixture needs an annotated tag");

    // `Head::id()` would answer with the tag here: it resolves a detached HEAD as
    // `peeled.unwrap_or(target)`, and `peeled` is only ever filled from a packed-refs `^` line.
    assert_eq!(tagged.head, Head::Detached { id: commit });
}

#[test]
fn the_tip_commit_uses_author_time_and_the_message_summary() {
    let rows = rows();
    let synced = row(&rows, "synced");

    let commit = synced.last_commit.as_ref().expect("a tip commit");
    assert_eq!(
        commit.id,
        fixtures::git_out(&synced.path, &["rev-parse", "HEAD"])
    );
    assert_eq!(commit.summary, "fixture");
    assert_eq!(commit.author, "repo-viewer tests");

    let author_seconds: u64 = fixtures::git_out(&synced.path, &["log", "-1", "--format=%at"])
        .parse()
        .expect("author time");
    assert_eq!(commit.time_ms, author_seconds * 1_000);
}

#[test]
fn an_unborn_head_has_no_tip_commit() {
    let rows = rows();
    let unborn = row(&rows, "unborn");

    assert_eq!(unborn.head, Head::Unborn);
    assert_eq!(unborn.last_commit, None);
    assert_eq!(unborn.error, None, "an unborn branch is not a failure");
}

#[test]
fn stash_entries_are_counted() {
    let rows = rows();

    assert_eq!(row(&rows, "stashed").stash_count, 2);
    assert_eq!(
        row(&rows, "synced").stash_count,
        0,
        "a repository that has never stashed has no refs/stash at all"
    );
}

#[test]
fn every_in_progress_state_is_mapped() {
    let rows = rows();

    assert_eq!(row(&rows, "synced").state, RepoState::Clean);
    assert_eq!(row(&rows, "merging").state, RepoState::Merging);
    assert_eq!(row(&rows, "bisecting").state, RepoState::Bisecting);
    assert_eq!(row(&rows, "cherrypicking").state, RepoState::CherryPicking);
    assert_eq!(row(&rows, "rebasing").state, RepoState::Rebasing);

    // The variant the model could not express before: gix reports Revert and RevertSequence, and
    // reporting either as Clean would hide an operation the user has to finish or abort.
    assert_eq!(row(&rows, "reverting").state, RepoState::Reverting);
}

#[test]
fn last_fetched_comes_from_the_common_directory() {
    let rows = rows();

    // A repository that has fetched has a FETCH_HEAD.
    assert!(
        row(&rows, "behind").last_fetched_ms.is_some(),
        "behind/ was fetched during fixture setup"
    );

    // A linked worktree's own Git directory is `worktrees/<name>` and never holds FETCH_HEAD, so
    // reading `git_dir()` instead of `common_dir()` would report "never fetched" forever.
    assert!(
        row(&rows, "wt").last_fetched_ms.is_some(),
        "a linked worktree shares its parent's FETCH_HEAD"
    );

    // And one that never fetched reports unknown rather than a zero timestamp.
    assert_eq!(row(&rows, "noupstream").last_fetched_ms, None);
}

#[test]
fn a_linked_worktree_has_its_own_head() {
    let rows = rows();
    let worktree = row(&rows, "wt");

    assert_eq!(worktree.kind, RepoKind::LinkedWorktree);
    assert_eq!(
        worktree.head,
        Head::Branch {
            name: "wt".to_string()
        },
        "the worktree is on its own branch while ahead/ stays on main"
    );
    assert_eq!(
        row(&rows, "ahead").head,
        Head::Branch {
            name: "main".to_string()
        }
    );
}

#[test]
fn a_bare_repository_reads_tier_0_and_leaves_the_later_tiers_unknown() {
    let rows = rows();
    let bare = row(&rows, "bare.git");

    assert_eq!(bare.kind, RepoKind::Bare);
    assert_eq!(bare.state, RepoState::Clean);
    assert_eq!(bare.stash_count, 0);
    assert_eq!(bare.error, None);

    // No worktree means these can never be computed for this row, and they must say so rather
    // than reporting zero counts.
    assert_eq!(bare.dirty, None);
    assert_eq!(bare.conflicted, None);
    assert_eq!(bare.counts, None);
}

#[test]
fn tier_0_leaves_every_later_tier_unknown() {
    let rows = rows();

    for (name, status) in &rows {
        assert_eq!(status.dirty, None, "`{name}` computed a Tier 1 dirty flag");
        assert_eq!(status.conflicted, None, "`{name}` computed a Tier 1 count");
        assert_eq!(status.counts, None, "`{name}` computed Tier 2 counts");
        // Reading the submodule list means reading `.gitmodules` from the worktree, with a full
        // index parse as its fallback. Tier 0 must not, so this stays unknown even for the
        // repositories that have submodules.
        assert_eq!(
            status.submodules, None,
            "`{name}` computed a Tier 2 submodule list"
        );
    }
}

#[test]
fn a_repository_that_cannot_be_opened_produces_no_row() {
    let tree = fixtures::status_tree();

    // Hand-built rather than discovered: discovery would never hand Tier 0 a non-repository, and
    // this is the path a repository corrupted between the walk and the read takes.
    let bogus = DiscoveredRepo {
        path: tree.path("notrepo"),
        name: "notrepo".to_string(),
        parent: tree.root().to_path_buf(),
        kind: RepoKind::Normal,
        git_dir: tree.path("notrepo/.git"),
    };

    let failure = read_tier0(&bogus).expect_err("a non-repository cannot produce a row");
    let message = failure.to_string();
    assert!(
        message.contains("notrepo"),
        "the failure should name the path, got: {message}"
    );

    // And the fan-out records it as a value rather than losing the pass over it.
    let (statuses, summary) = read_tier0_all(&[bogus], &AtomicBool::new(false));
    assert!(statuses.is_empty());
    assert_eq!(summary.repos_read, 0);
    assert_eq!(summary.errors.len(), 1);
}

#[test]
fn cancellation_stops_the_pass() {
    let tree = fixtures::status_tree();
    let (found, _) = discover_roots(&[tree.root().to_path_buf()], &ScanOpts::default());
    assert!(
        !found.is_empty(),
        "the fixture tree should hold repositories"
    );

    // Pre-set so the outcome does not depend on winning a race with the fan-out.
    let cancelled = AtomicBool::new(true);
    let (statuses, summary) = read_tier0_all(&found, &cancelled);

    assert!(statuses.is_empty());
    assert_eq!(summary.repos_read, 0);
    assert!(
        summary.errors.is_empty(),
        "cancelling is not a failure: {:?}",
        summary.errors
    );
}

#[test]
fn rows_come_back_sorted_by_path() {
    let tree = fixtures::status_tree();
    let (found, _) = discover_roots(&[tree.root().to_path_buf()], &ScanOpts::default());
    let (statuses, _) = read_tier0_all(&found, &AtomicBool::new(false));

    let mut sorted = statuses.clone();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    let paths: Vec<_> = statuses.iter().map(|status| &status.path).collect();
    let expected: Vec<_> = sorted.iter().map(|status| &status.path).collect();
    assert_eq!(
        paths, expected,
        "the fan-out completes out of order, so the collecting variant sorts"
    );
}
