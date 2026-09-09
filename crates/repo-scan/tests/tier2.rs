//! Tier 2 against a real tree: the four column counts and the submodule list.
//!
//! Every count test here has both a literal and an oracle, and the pair is the point. A literal
//! encodes what the author believed the fixture contained; `git status --porcelain` cannot drift
//! from git's own behaviour. Tier 1's suite needed only one of each because it reports a boolean —
//! four numbers have four ways to be individually right and collectively wrong, of which swapping
//! two columns is the one a single-change fixture can never catch.

mod support;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use repo_scan::{
    DiscoveredRepo, FileCounts, RepoKind, ScanOpts, Tier2, discover_roots, read_tier1, read_tier2,
};

use support::fixtures;

/// Every repository in the status tree, as discovery reports them.
fn discovered() -> Vec<DiscoveredRepo> {
    let tree = fixtures::status_tree();
    let (found, _) = discover_roots(
        &[tree.root().to_path_buf()],
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );
    found
}

/// The `DiscoveredRepo` for a named fixture repository.
fn find(found: &[DiscoveredRepo], tree_path: &Path) -> DiscoveredRepo {
    found
        .iter()
        .find(|repo| repo.path == tree_path)
        .unwrap_or_else(|| {
            panic!(
                "no discovered repository at {}; got {:?}",
                tree_path.display(),
                found.iter().map(|repo| &repo.path).collect::<Vec<_>>()
            )
        })
        .clone()
}

/// Read Tier 2 for one named fixture repository.
fn read(name: &str) -> Tier2 {
    let tree = fixtures::status_tree();
    let found = discovered();
    let repo = find(&found, &tree.path(name));

    read_tier2(&repo, &Arc::new(AtomicBool::new(false)))
        .unwrap_or_else(|err| panic!("Tier 2 failed for {name}: {err}"))
        .unwrap_or_else(|| panic!("{name} was skipped as having no worktree"))
}

/// The counts for one named fixture repository, which must have been read.
fn counts(name: &str) -> FileCounts {
    read(name)
        .counts
        .unwrap_or_else(|| panic!("no counts for {name}"))
}

// -------------------------------------------------------------------------------------------
// The four columns
// -------------------------------------------------------------------------------------------

/// One change of each kind at once, so no two columns can be swapped undetected.
#[test]
fn one_change_of_each_kind_lands_in_its_own_column() {
    assert_eq!(
        counts("counts"),
        FileCounts {
            staged: 1,
            unstaged: 1,
            untracked: 1,
            conflicted: 0,
        }
    );
}

/// **The `IntentToAdd` trap.** `git add -N` records an index entry promising content the object
/// database does not hold, and git counts that as **unstaged only**: `git status --porcelain`
/// prints ` A`, with the index column empty. Reading the `A` as a staged addition is the mistake,
/// and it is an easy one, because the entry genuinely is in the index.
#[test]
fn an_intent_to_add_path_counts_as_unstaged_only() {
    assert_eq!(
        counts("intentadd"),
        FileCounts {
            staged: 0,
            unstaged: 1,
            untracked: 0,
            conflicted: 0,
        },
        "git prints ` A` for an intent-to-add path: worktree column only"
    );
}

/// **The per-column rule.** One file staged and then modified again is seen by both comparisons —
/// `MM` to `git status`, one staged plus one unstaged here. So the four counts are per-column
/// totals rather than a partition of paths, and nothing may sum them or call them a file count.
#[test]
fn one_path_can_count_in_two_columns_at_once() {
    let counted = counts("stagedthenmodified");

    assert_eq!(counted.staged, 1, "HEAD's tree differs from the index");
    assert_eq!(
        counted.unstaged, 1,
        "and the index differs from the worktree"
    );
    assert_eq!(
        counted.staged + counted.unstaged,
        2,
        "two column entries for one path — the sum is not a file count"
    );
}

/// **The rename trap.** Tree-index rename tracking is on by default, so a staged rename arrives as
/// one `Rewrite` change spanning two paths. Counting its paths reports 2 where `git status` prints
/// a single `R old -> new` line.
#[test]
fn a_staged_rename_counts_once_not_twice() {
    assert_eq!(
        counts("renamed"),
        FileCounts {
            staged: 1,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
        }
    );
}

/// **The collapsed-untracked trap.** `UntrackedFiles::Collapsed` reports a wholly untracked
/// directory as one entry, which is what `git status` shows; `Files` would report three here.
#[test]
fn an_untracked_directory_counts_as_one_entry_not_one_per_file() {
    assert_eq!(
        counts("untrackeddir").untracked,
        1,
        "the directory is one entry, as `git status` reports it"
    );
}

/// A conflicted path has up to three index entries and is still one file.
#[test]
fn a_conflicted_merge_counts_files_not_index_entries() {
    assert_eq!(counts("conflicted").conflicted, 2);
}

/// Tier 1 and Tier 2 reach the conflicted count by different routes — index stage entries against
/// the status iterator's `Conflict` items — and must agree. Tier 1 owns the row's `conflicted`
/// field; this is what makes `counts.conflicted` a cross-check rather than a second answer.
#[test]
fn the_two_tiers_agree_on_the_conflicted_count() {
    let tree = fixtures::status_tree();
    let found = discovered();
    let repo = find(&found, &tree.path("conflicted"));
    let flag = Arc::new(AtomicBool::new(false));

    let tier1 = read_tier1(&repo, &flag)
        .expect("Tier 1 reads the conflicted fixture")
        .expect("it has a worktree");
    let tier2 = read_tier2(&repo, &flag)
        .expect("Tier 2 reads the conflicted fixture")
        .expect("it has a worktree")
        .counts
        .expect("with counts");

    assert_eq!(tier1.conflicted, tier2.conflicted);
}

/// A clean clone has nothing in any column — four computed zeros, which is a different fact from
/// four unknowns and is why the row's fields are `Option` and these are not.
#[test]
fn an_untouched_clone_counts_zero_in_every_column() {
    assert_eq!(counts("synced"), FileCounts::default());
}

// -------------------------------------------------------------------------------------------
// Submodules
// -------------------------------------------------------------------------------------------

/// `submodules()` yielding `Ok(None)` means "no submodule configuration", which is an answered
/// question. Reporting it as `None` would claim a read is still outstanding for the overwhelmingly
/// common case — the uncomputed-renders-as-zero bug wearing the submodule list's clothes.
#[test]
fn a_repository_with_no_submodules_reports_an_empty_list_not_an_unknown_one() {
    assert_eq!(read("submodnone").submodules, Some(Vec::new()));
}

#[test]
fn a_recorded_submodule_is_listed_with_its_path_and_ids() {
    let listed = read("submodparent").submodules.expect("the list was read");

    assert_eq!(listed.len(), 1, "got {listed:?}");
    let sub = &listed[0];
    assert_eq!(sub.path, PathBuf::from("sub"));
    assert_eq!(sub.name, "sub");
    assert!(
        sub.recorded_id.is_some(),
        "the parent's index records a commit for it"
    );
    assert!(
        sub.head_id.is_some(),
        "and it is checked out, so it has its own HEAD"
    );
}

// -------------------------------------------------------------------------------------------
// Grades of absence
// -------------------------------------------------------------------------------------------

/// A bare repository has no worktree and no `.gitmodules`, so neither question applies. Skipped
/// rather than reported as zero: "cannot ever" is not "not yet", and the row's `None` fields plus
/// its `kind` are what let the UI say `n/a`.
#[test]
fn a_bare_repository_is_skipped_rather_than_counted_as_clean() {
    let tree = fixtures::status_tree();
    let found = discovered();
    let bare = find(&found, &tree.path("bare.git"));
    assert_eq!(bare.kind, RepoKind::Bare, "the fixture is bare");

    let read =
        read_tier2(&bare, &Arc::new(AtomicBool::new(false))).expect("skipping is not a failure");

    assert!(read.is_none());
}

/// A linked worktree has its own index and worktree, so it reads Tier 2 like any other row.
#[test]
fn a_linked_worktree_reads_tier_2() {
    let tree = fixtures::status_tree();
    let found = discovered();
    let worktree = find(&found, &tree.path("wt"));
    assert_eq!(worktree.kind, RepoKind::LinkedWorktree);

    let read = read_tier2(&worktree, &Arc::new(AtomicBool::new(false)))
        .expect("a linked worktree reads")
        .expect("and is not skipped");

    assert!(read.counts.is_some());
    assert_eq!(read.error, None);
}

/// A pre-flipped interrupt stops the walk. `gix` reports an interrupted status as an error rather
/// than as an empty result, so the half that failed is `None` with the cause recorded — the row
/// survives, which is the partial grade.
#[test]
fn cancellation_leaves_the_counts_unknown_rather_than_zero() {
    let tree = fixtures::status_tree();
    let found = discovered();
    let repo = find(&found, &tree.path("counts"));

    let flag = Arc::new(AtomicBool::new(false));
    flag.store(true, Ordering::Relaxed);
    let read = read_tier2(&repo, &flag)
        .expect("cancelling is not a total failure")
        .expect("the repository has a worktree");

    // Either the walk was interrupted before producing anything, in which case the counts are
    // unknown and the cause is recorded, or it finished before the flag was observed. What must
    // never happen is unknown counts presented as zeros.
    if read.counts.is_none() {
        assert!(read.error.is_some(), "an absent count says why");
    }
}

// -------------------------------------------------------------------------------------------
// The oracle
// -------------------------------------------------------------------------------------------

/// Every count agrees with `git status --porcelain` on every repository in the tree.
///
/// The literals above stay as well: an implementation that returned the porcelain parse verbatim
/// would pass this and fail those, and one that hard-coded the fixtures would pass those and fail
/// this.
#[test]
fn the_counts_match_git_status() {
    let flag = Arc::new(AtomicBool::new(false));

    for repo in &discovered() {
        if repo.kind == RepoKind::Bare {
            continue;
        }

        let read = read_tier2(repo, &flag)
            .unwrap_or_else(|err| panic!("Tier 2 failed for {}: {err}", repo.path.display()))
            .expect("a non-bare repository is not skipped");
        let counted = read
            .counts
            .unwrap_or_else(|| panic!("no counts for {}", repo.path.display()));

        // Untrimmed: porcelain's leading space is the index column, and trimming the first line
        // moves an unstaged change into the staged count.
        let porcelain = fixtures::git_out_raw(&repo.path, &["status", "--porcelain"]);
        let expected = parse_porcelain(&porcelain);

        assert_eq!(
            counted,
            expected,
            "counts disagree with `git status --porcelain` for {}:\n{porcelain}",
            repo.path.display()
        );
    }
}

/// Turn `git status --porcelain` into the four column totals.
///
/// Porcelain's first two characters are the index status and the worktree status of one path, which
/// is the same split Tier 2 counts — so a path can contribute to two columns, and `AM` does. The
/// unmerged codes are the exception: git defines seven two-letter combinations that all mean
/// "conflicted", and for those the columns describe which sides conflict rather than staged and
/// unstaged work.
fn parse_porcelain(output: &str) -> FileCounts {
    let mut counts = FileCounts::default();

    for line in output.lines() {
        if line.len() < 2 {
            continue;
        }
        let mut chars = line.chars();
        let index = chars.next().unwrap_or(' ');
        let worktree = chars.next().unwrap_or(' ');

        match (index, worktree) {
            // Untracked, and ignored if it were ever asked for.
            ('?', '?') => counts.untracked += 1,
            ('!', '!') => {}
            // The seven unmerged combinations. Counted once, in one column.
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => counts.conflicted += 1,
            _ => {
                if index != ' ' {
                    counts.staged += 1;
                }
                if worktree != ' ' {
                    counts.unstaged += 1;
                }
            }
        }
    }

    counts
}
