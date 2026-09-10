//! Fetch, against a real `git` and a real local origin.
//!
//! Every test builds its **own** origin and its own clones. A fetch mutates the repository it runs
//! in, so sharing a tree the way `tier0.rs` and `tier1.rs` share `status_tree()` would have these
//! tests moving each other's ahead/behind counts — and cargo runs the tests in one binary in
//! parallel, so it would be intermittent rather than obvious.
//!
//! **Nothing here touches the network.** The origin is a bare repository on disk, reached by path;
//! the one unreachable-remote test points at loopback port 1, where a connection is refused with
//! no DNS lookup and no packets leaving the machine.

mod support;

use std::{path::Path, sync::atomic::AtomicBool, time::Duration};

use repo_scan::{
    DiscoveredRepo, FetchOpts, FetchStatus, RepoKind, ScanOpts, discover_roots, fetch_all_with,
    fetch_one, probe_git, read_tier0,
};
use support::fixtures;

/// Discover one repository by path, the way a scan would.
///
/// Through discovery rather than hand-built, so the tests exercise the same `git_dir` and
/// `common_dir` resolution the real path uses — which is the whole reason fetch does not
/// re-resolve either.
fn discovered(path: &Path) -> DiscoveredRepo {
    let opts = ScanOpts {
        max_depth: Some(2),
        ..ScanOpts::default()
    };
    let (found, summary) = discover_roots(&[path.to_path_buf()], &opts, &AtomicBool::new(false));
    assert!(
        summary.errors.is_empty(),
        "discovery of {} reported errors: {:?}",
        path.display(),
        summary.errors
    );
    found
        .into_iter()
        .find(|repo| repo.path == path)
        .unwrap_or_else(|| panic!("{} was not discovered", path.display()))
}

/// Fetch options that actually run `git`, with no repeat guard.
fn opts() -> FetchOpts {
    FetchOpts {
        timeout: Duration::from_secs(60),
        ..FetchOpts::default()
    }
}

fn fetch(repo: &DiscoveredRepo, opts: &FetchOpts) -> repo_scan::FetchOutcome {
    fetch_one(repo, opts, &AtomicBool::new(false))
}

/// The assumption the entire last-fetched column rests on.
///
/// `last_fetched_ms` is `FETCH_HEAD`'s mtime. If `git fetch --all` ever stopped writing that file,
/// a successful fetch would leave the row reading "never fetched" — a fresh lie in the one field
/// this whole phase exists to make honest. The fallback if this ever fails is to stop passing
/// `--all`, never to synthesise a timestamp: the engine must not write into a user's repository to
/// make a display field true.
#[test]
fn a_fetch_writes_fetch_head_so_the_row_can_report_an_age() {
    let tree = fixtures::fetch_tree();
    let repo = discovered(&tree.clone_origin("alpha"));

    assert!(
        read_tier0(&repo).expect("a row").last_fetched_ms.is_none(),
        "a fresh clone has not fetched"
    );

    let result = fetch(&repo, &opts());

    assert_eq!(
        result.status,
        FetchStatus::Ok,
        "detail: {:?}",
        result.detail
    );
    assert!(
        read_tier0(&repo).expect("a row").last_fetched_ms.is_some(),
        "the fetch must leave an age the table can show"
    );
}

/// The deliverable, end to end and with no mock anywhere: a fetch changes what Tier 0 reports.
#[test]
fn a_fetch_moves_the_behind_count() {
    let tree = fixtures::fetch_tree();
    let repo = discovered(&tree.clone_origin("alpha"));

    assert_eq!(read_tier0(&repo).expect("a row").behind, Some(0));

    tree.advance_origin(3);
    let result = fetch(&repo, &opts());

    assert_eq!(
        result.status,
        FetchStatus::Ok,
        "detail: {:?}",
        result.detail
    );
    assert_eq!(
        read_tier0(&repo).expect("a row").behind,
        Some(3),
        "the whole point of fetching"
    );
}

/// `--prune` is not optional: without it a deleted remote branch leaves its tracking ref forever
/// and `behind` keeps counting against a ref that no longer exists upstream.
///
/// The second half is the guard on its destructive neighbour. `--prune-tags` would delete the
/// user's own local tags, which is outside "read-only plus fetch" and unrecoverable — so a real
/// fetch is asserted to leave a local tag alone, not just the argv.
#[test]
fn prune_drops_a_dead_tracking_ref_and_leaves_local_tags_alone() {
    let tree = fixtures::fetch_tree();
    tree.push_branch("doomed");
    let path = tree.clone_origin("alpha");
    let repo = discovered(&path);

    fixtures::git_out(&path, &["tag", "my-local-tag"]);
    assert!(
        tree.git_out(&path, &["branch", "-r"])
            .contains("origin/doomed"),
        "the clone starts with the tracking ref"
    );

    tree.delete_branch("doomed");
    let result = fetch(&repo, &opts());

    assert_eq!(
        result.status,
        FetchStatus::Ok,
        "detail: {:?}",
        result.detail
    );
    assert!(
        !tree
            .git_out(&path, &["branch", "-r"])
            .contains("origin/doomed"),
        "a tracking ref for a branch that is gone must not survive"
    );
    assert!(
        tree.git_out(&path, &["tag"]).contains("my-local-tag"),
        "a local tag is the user's own object and is not the remote's to delete"
    );
}

/// The most common non-success in a real tree, and the one that must never reach prose matching.
///
/// Staged with a program that could not possibly run, so a spawn would be unmistakable: the only
/// way this can report `NoRemote` is if the pre-flight answered before reaching the process.
#[test]
fn a_repository_with_no_remote_is_answered_without_spawning_anything() {
    let tree = fixtures::fetch_tree();
    let repo = discovered(&tree.init_without_remote("lonely"));

    let result = fetch_one(
        &repo,
        &FetchOpts {
            program: "definitely-not-a-real-program".into(),
            ..opts()
        },
        &AtomicBool::new(false),
    );

    assert_eq!(
        result.status,
        FetchStatus::NoRemote,
        "a bogus program would have produced GitMissing had anything been spawned"
    );
    assert_eq!(result.elapsed_ms, 0);
}

/// An unreachable remote, with no DNS and no packets off the machine.
#[test]
fn an_unreachable_remote_reads_as_a_network_failure() {
    let tree = fixtures::fetch_tree();
    let path = tree.clone_origin("alpha");
    tree.break_remote(&path);
    let repo = discovered(&path);

    let result = fetch(
        &repo,
        &FetchOpts {
            timeout: Duration::from_secs(20),
            ..opts()
        },
    );

    assert_eq!(
        result.status,
        FetchStatus::Network,
        "detail: {:?}",
        result.detail
    );
    assert!(
        result.detail.is_some_and(|detail| !detail.is_empty()),
        "git's own words are what the user reads"
    );
}

/// §8.2's concurrency cap, which is otherwise entirely unobserved.
///
/// The callback tracks live processes as a high-water mark rather than counting starts, because
/// four starts spread over a minute is not four concurrent fetches.
#[test]
fn no_more_than_the_cap_run_at_once() {
    use std::sync::{Arc, Mutex};

    let tree = fixtures::fetch_tree();
    let repos: Vec<DiscoveredRepo> = (0..12)
        .map(|index| discovered(&tree.clone_origin(&format!("clone-{index}"))))
        .collect();

    let live = Arc::new(Mutex::new(0_i32));
    let peak = Arc::new(Mutex::new(0_i32));

    let summary = fetch_all_with(
        &repos,
        &FetchOpts {
            concurrency: 2,
            ..opts()
        },
        &AtomicBool::new(false),
        |notice| {
            let mut live = live.lock().expect("not poisoned");
            match notice {
                repo_scan::FetchNotice::Started(_) => *live += 1,
                repo_scan::FetchNotice::Done(_) => *live -= 1,
            }
            let mut peak = peak.lock().expect("not poisoned");
            *peak = (*peak).max(*live);
        },
    );

    assert_eq!(summary.succeeded, 12, "every clone fetches");
    let peak = *peak.lock().expect("not poisoned");
    assert!(peak <= 2, "the cap is 2, but {peak} ran at once");
    assert!(peak >= 1, "something ran");
}

/// Window close must stop the queue, not merely stop starting new work.
///
/// The bogus program is what proves nothing ran: had any repository been reached, it would have
/// come back `GitMissing` rather than `Cancelled`.
#[test]
fn an_interrupt_set_before_the_pass_runs_nothing() {
    let tree = fixtures::fetch_tree();
    let repos: Vec<DiscoveredRepo> = (0..4)
        .map(|index| discovered(&tree.clone_origin(&format!("clone-{index}"))))
        .collect();

    let outcomes = std::sync::Mutex::new(Vec::new());
    let summary = fetch_all_with(
        &repos,
        &FetchOpts {
            program: "definitely-not-a-real-program".into(),
            ..opts()
        },
        &AtomicBool::new(true),
        |notice| {
            if let repo_scan::FetchNotice::Done(result) = notice {
                outcomes.lock().expect("not poisoned").push(result);
            }
        },
    );

    assert!(summary.cancelled);
    assert_eq!(summary.attempted, 0, "no process was spawned");
    assert_eq!(summary.skipped, 4);
    let outcomes = outcomes.into_inner().expect("not poisoned");
    assert_eq!(outcomes.len(), 4, "every repository is still accounted for");
    assert!(
        outcomes
            .iter()
            .all(|result| result.status == FetchStatus::Cancelled),
        "got {:?}",
        outcomes.iter().map(|r| r.status).collect::<Vec<_>>()
    );
}

/// A cancellation part-way through stops the rest, which is what makes a 300-repository fetch
/// survivable when the user closes the window.
#[test]
fn an_interrupt_mid_pass_stops_the_repositories_after_it() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool as Flag, Ordering},
    };

    let tree = fixtures::fetch_tree();
    let repos: Vec<DiscoveredRepo> = (0..8)
        .map(|index| discovered(&tree.clone_origin(&format!("clone-{index}"))))
        .collect();

    let flag = Arc::new(Flag::new(false));
    let raised = Arc::clone(&flag);
    let cancelled = std::sync::Mutex::new(0_u32);

    let summary = fetch_all_with(
        &repos,
        &FetchOpts {
            concurrency: 1,
            ..opts()
        },
        &flag,
        |notice| {
            if let repo_scan::FetchNotice::Done(result) = notice {
                // Stop after the first repository settles.
                raised.store(true, Ordering::Relaxed);
                if result.status == FetchStatus::Cancelled {
                    *cancelled.lock().expect("not poisoned") += 1;
                }
            }
        },
    );

    assert!(summary.cancelled);
    assert!(
        summary.attempted < 8,
        "the pass stopped early, attempted {}",
        summary.attempted
    );
    assert!(
        *cancelled.lock().expect("not poisoned") > 0,
        "the repositories after the interrupt are reported, not dropped"
    );
}

/// The repeat guard applies to a bulk pass and never to a single request.
#[test]
fn the_repeat_guard_skips_a_bulk_refetch_and_never_a_single_one() {
    let tree = fixtures::fetch_tree();
    let repo = discovered(&tree.clone_origin("alpha"));

    let guarded = FetchOpts {
        min_interval: Some(Duration::from_secs(3_600)),
        ..opts()
    };

    assert_eq!(fetch(&repo, &guarded).status, FetchStatus::Ok);
    assert_eq!(
        fetch(&repo, &guarded).status,
        FetchStatus::TooSoon,
        "a second bulk pass must not fetch it again"
    );
    assert_eq!(
        fetch(&repo, &opts()).status,
        FetchStatus::Ok,
        "a user clicking one repository's button is expressing intent"
    );
}

/// A bare repository is fetched at its own directory. An implementation that passed an explicit
/// `--work-tree` instead of `-C` gets exactly this case wrong.
#[test]
fn a_bare_repository_fetches() {
    let tree = fixtures::fetch_tree();
    let path = tree.clone_origin_bare("mirror.git");
    let repo = discovered(&path);
    assert_eq!(repo.kind, RepoKind::Bare, "the fixture is bare");

    let result = fetch(&repo, &opts());

    assert_eq!(
        result.status,
        FetchStatus::Ok,
        "detail: {:?}",
        result.detail
    );
}

/// Fetching in a linked worktree moves the **parent's** ahead/behind, because the
/// `refs/remotes/*` it writes live in the shared common directory.
///
/// The premise the sibling refresh rests on, asserted rather than assumed: the parent has no idea
/// a fetch happened, so if this were false its row would sit stale with nothing to correct it.
#[test]
fn fetching_a_worktree_moves_its_parents_counts_too() {
    let tree = fixtures::fetch_tree();
    let parent_path = tree.clone_origin("alpha");
    fixtures::git_out(&parent_path, &["worktree", "add", "-b", "side", "../wt"]);
    let parent = discovered(&parent_path);
    let worktree = discovered(&tree.path("wt"));
    assert_eq!(worktree.kind, RepoKind::LinkedWorktree);
    assert_ne!(
        worktree.git_dir, worktree.common_dir,
        "a linked worktree's two directories differ, which is what makes this a real case"
    );

    tree.advance_origin(2);
    let result = fetch(&worktree, &opts());

    assert_eq!(
        result.status,
        FetchStatus::Ok,
        "detail: {:?}",
        result.detail
    );
    assert_eq!(
        read_tier0(&parent).expect("a row").behind,
        Some(2),
        "the remote-tracking refs are shared, so the parent moved without being fetched itself"
    );
}

/// `FETCH_HEAD` is written to the git directory of whichever worktree ran the fetch — **not**
/// always the common one.
///
/// Reading only the common directory reports "never fetched" for a worktree fetched a second ago,
/// which is a fresh lie in the one field whose job is to say how stale the counts beside it are.
/// Both halves are asserted because the rule that only the common directory matters is true of an
/// un-fetched worktree and false the moment that worktree is fetched itself.
#[test]
fn a_worktree_fetch_is_visible_as_an_age_even_though_it_writes_privately() {
    let tree = fixtures::fetch_tree();
    let parent_path = tree.clone_origin("alpha");
    fixtures::git_out(&parent_path, &["worktree", "add", "-b", "side", "../wt"]);
    let worktree = discovered(&tree.path("wt"));

    let result = fetch(&worktree, &opts());
    assert_eq!(
        result.status,
        FetchStatus::Ok,
        "detail: {:?}",
        result.detail
    );

    assert!(
        worktree.git_dir.join("FETCH_HEAD").is_file(),
        "git writes it into the worktree's own directory"
    );
    assert!(
        !worktree.common_dir.join("FETCH_HEAD").is_file(),
        "and not into the common one, which is why reading only there misses this fetch"
    );
    assert!(
        read_tier0(&worktree)
            .expect("a row")
            .last_fetched_ms
            .is_some(),
        "the row still has to report an age, or the fetch it just did is invisible"
    );
}

/// The other direction of the same rule: a fetch in the parent is visible from the worktree,
/// which has no `FETCH_HEAD` of its own.
#[test]
fn a_parents_fetch_is_visible_from_its_worktree() {
    let tree = fixtures::fetch_tree();
    let parent_path = tree.clone_origin("alpha");
    fixtures::git_out(&parent_path, &["worktree", "add", "-b", "side", "../wt"]);
    let parent = discovered(&parent_path);
    let worktree = discovered(&tree.path("wt"));

    assert_eq!(fetch(&parent, &opts()).status, FetchStatus::Ok);

    assert!(
        !worktree.git_dir.join("FETCH_HEAD").is_file(),
        "the worktree ran no fetch of its own"
    );
    assert!(
        read_tier0(&worktree)
            .expect("a row")
            .last_fetched_ms
            .is_some(),
        "it shares the refs, so it shares their age"
    );
}

/// The startup probe, which is free here because the fixtures already require `git` on `PATH`.
#[test]
fn the_probe_finds_the_git_the_fixtures_are_using() {
    let info = probe_git().expect("git is on PATH — the fixtures need it too");

    assert!(
        info.version.starts_with("git version"),
        "got {:?}",
        info.version
    );
    assert!(!info.path.as_os_str().is_empty());
}
