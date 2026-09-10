//! Watching against a real tree.
//!
//! Two halves, and they are worth telling apart. The [`watch_set`] tests are ordinary assertions
//! over a pure function and a real fixture: no watcher exists, nothing is registered, and they fail
//! deterministically. The [`RepoWatcher`] tests drive an actual OS backend, so they wait for a
//! callback rather than asserting immediately — a debounce window has to elapse before anything can
//! arrive, and how long the kernel takes to deliver is not ours to control.
//!
//! Those waits are generous on purpose. A tight deadline here would produce a test that fails on a
//! loaded CI agent for reasons that have nothing to do with the code, which is worse than no test.

mod support;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, atomic::AtomicBool},
    time::{Duration, Instant},
};

use notify::RecursiveMode;
use repo_scan::{
    DiscoveredRepo, RepoWatcher, ScanOpts, WatchEvent, discover_roots, watch::watch_set,
};

use support::fixtures;

/// The debounce window the watcher tests use.
///
/// Shorter than the app's ~400 ms so the tests are not paced by it, and still long enough to do
/// its job: `git`'s `index.lock` burst arrives inside a few milliseconds.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// How long a test waits for a change to be reported.
const DEADLINE: Duration = Duration::from_secs(15);

/// The deliverable of the set: three places inside the Git directory, and never the worktree.
///
/// Recursion is asserted per entry rather than only membership, because it is the half that gets
/// this wrong silently. A non-recursive `refs/` watch sees only its direct children, so it would
/// miss `refs/heads/feature/x` and every `refs/remotes/origin/*` update — which is the ahead/behind
/// signal, and the thing this app exists to report.
#[test]
fn the_watch_set_is_the_git_dir_root_refs_recursively_and_the_head_log() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);
    let plain = row(&repos, &fixture.path("plain"));

    let watches = watch_set(plain);
    let git_dir = fixture.path("plain/.git");

    assert_eq!(
        mode_of(&watches, &git_dir),
        Some(RecursiveMode::NonRecursive),
        "the git dir root is watched, and not recursively — `objects/` is underneath it"
    );
    assert_eq!(
        mode_of(&watches, &git_dir.join("refs")),
        Some(RecursiveMode::Recursive),
        "`refs/` must be recursive or slash-named and remote-tracking refs are invisible"
    );
    assert_eq!(
        mode_of(&watches, &git_dir.join("logs").join("HEAD")),
        Some(RecursiveMode::NonRecursive),
        "`logs/HEAD` is appended on every commit, checkout and reset"
    );

    assert!(
        !watches
            .iter()
            .any(|(path, _)| path == &fixture.path("plain")),
        "the worktree itself is never watched: recursively watching worktrees means recursively \
         watching node_modules"
    );
}

/// A linked worktree contributes from **both** directories. Its private git dir holds its own
/// `HEAD` and `index`; the refs and `FETCH_HEAD` it reads live in the common one.
#[test]
fn a_linked_worktree_watches_its_private_and_its_common_directory() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);
    let worktree = row(&repos, &fixture.path("wt"));

    let watches = watch_set(worktree);
    let common = fixture.path("plain/.git");

    assert_eq!(
        mode_of(&watches, &worktree.git_dir),
        Some(RecursiveMode::NonRecursive),
        "its own HEAD and index live in the private directory"
    );
    assert_eq!(
        mode_of(&watches, &common.join("refs")),
        Some(RecursiveMode::Recursive),
        "and its refs live in the common one — the case the `common_dir` field exists for"
    );
    assert_eq!(
        mode_of(&watches, &worktree.git_dir.join("refs")),
        Some(RecursiveMode::Recursive),
        "the private directory has a `refs/` of its own too, and it is not decoration: \
         per-worktree refs — `refs/bisect/*` and `refs/worktree/*` — live there rather than in \
         the common directory, so a bisect in this worktree is only visible here"
    );
}

/// A repository with no commits has no `logs/HEAD`, and `notify`'s `watch()` fails with
/// `PathNotFound` on a path that is not there. So the set is filtered by existence, or every
/// freshly `init`ed repository in a tree would report a registration failure.
///
/// `plain/nested` is the fixture's only `git init` with nothing committed, which is exactly this
/// case.
#[test]
fn a_repository_with_no_commits_contributes_no_head_log() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);
    let fresh = row(&repos, &fixture.path("plain/nested"));

    let log = fresh.git_dir.join("logs").join("HEAD");
    assert!(
        !log.exists(),
        "the fixture's premise: git writes this on the first ref update, not at init"
    );

    let watches = watch_set(fresh);
    assert!(
        mode_of(&watches, &log).is_none(),
        "so it is not in the set, and registration does not fail on it"
    );
    assert_eq!(
        watches.len(),
        2,
        "leaving the git dir root and `refs/`: {watches:?}"
    );
}

/// `sync` registers every repository it is given, and the path count is the number that matters
/// against a platform's watch limit — lower than three per repository, because worktrees share.
#[test]
fn sync_registers_every_repository() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);

    let (mut watcher, _seen) = watching();
    let failures = watcher.sync(&repos);

    assert!(failures.is_empty(), "unexpected failures: {failures:?}");
    assert_eq!(
        watcher.watched_repos(),
        repos.len(),
        "every repository the walk found is watched"
    );
    assert!(
        watcher.watched_paths() < repos.len() * 3,
        "and the path count is below three per repository, because `plain` and `wt` share a \
         common directory and two repositories have no `logs/HEAD`: {} paths for {} repositories",
        watcher.watched_paths(),
        repos.len()
    );
}

/// The diff, both ways. A second sync with the same set changes nothing, and one with a repository
/// missing releases its paths.
#[test]
fn sync_is_idempotent_and_releases_what_is_gone() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);

    let (mut watcher, _seen) = watching();
    watcher.sync(&repos);
    let (first_repos, first_paths) = (watcher.watched_repos(), watcher.watched_paths());

    watcher.sync(&repos);
    assert_eq!(
        (watcher.watched_repos(), watcher.watched_paths()),
        (first_repos, first_paths),
        "re-syncing an unchanged set registers nothing new and releases nothing"
    );

    // Drop the bare repository, which shares nothing, so the release is unambiguous.
    let remaining: Vec<DiscoveredRepo> = repos
        .iter()
        .filter(|repo| repo.path != fixture.path("mirror.git"))
        .cloned()
        .collect();
    watcher.sync(&remaining);

    assert_eq!(watcher.watched_repos(), first_repos - 1);
    assert!(
        watcher.watched_paths() < first_paths,
        "its paths went with it"
    );
}

/// Removing a linked worktree must not release the refs its main repository still watches.
///
/// This is the reference-counting half of the index, and it is asserted as behaviour rather than as
/// a path count: what matters is that `plain` still reports, not how many handles are open. An index
/// keyed one-repository-per-path would have unwatched `plain/.git/refs` along with the worktree and
/// silently stopped reporting `plain`'s commits — a regression that no count would name.
#[test]
fn a_shared_path_survives_one_of_its_holders_leaving() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);

    let (mut watcher, seen) = watching();
    watcher.sync(&repos);

    let without_worktree: Vec<DiscoveredRepo> = repos
        .iter()
        .filter(|repo| repo.path != fixture.path("wt"))
        .cloned()
        .collect();
    watcher.sync(&without_worktree);

    write_ref(&fixture, "survivor");

    assert!(
        wait_for(&seen, &fixture.path("plain")),
        "`plain` shares `refs/` with the worktree that just left and must still be watching it; \
         saw {:?}",
        collected(&seen)
    );
    assert!(
        !collected(&seen).contains(&fixture.path("wt")),
        "and the worktree itself is released, so it is not reported"
    );
}

/// End to end: a ref written under a watched `refs/` is reported as its repository, by path.
///
/// A ref write rather than a `git commit`, because it is the smallest thing that produces a real
/// event inside the watch set — and because a full commit would additionally touch `index` and
/// `logs/HEAD`, which would let this pass even if `refs/` were not watched at all.
#[test]
fn a_ref_write_is_reported_as_its_repository() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);

    let (mut watcher, seen) = watching();
    let failures = watcher.sync(&repos);
    assert!(failures.is_empty(), "unexpected failures: {failures:?}");

    write_ref(&fixture, "watched");

    let changed = wait_for(&seen, &fixture.path("plain"));
    assert!(
        changed,
        "the write under `plain/.git/refs/` was never reported; saw {:?}",
        collected(&seen)
    );
}

/// And the same write is reported for the linked worktree too, because it is watching that
/// directory as its common one.
///
/// Both repositories genuinely changed: they share remotes, so a remote-tracking update there moves
/// both their ahead/behind counts. Reporting only one of them would leave the other's row stale with
/// nothing to correct it but the poll.
#[test]
fn a_write_in_a_shared_common_dir_is_reported_for_every_holder() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);

    let (mut watcher, seen) = watching();
    watcher.sync(&repos);

    write_ref(&fixture, "shared");

    assert!(
        wait_for(&seen, &fixture.path("wt")),
        "the worktree watching this directory as its common one was never reported; saw {:?}",
        collected(&seen)
    );
}

/// A repository that is no longer watched produces nothing, which is what makes `sync`'s release
/// half more than bookkeeping.
#[test]
fn a_released_repository_stops_being_reported() {
    let fixture = fixtures::build();
    let repos = discovered(&fixture);
    let bare = fixture.path("mirror.git");

    let (mut watcher, seen) = watching();
    watcher.sync(&repos);
    let remaining: Vec<DiscoveredRepo> = repos
        .iter()
        .filter(|repo| repo.path != bare)
        .cloned()
        .collect();
    watcher.sync(&remaining);

    std::fs::write(bare.join("refs/heads/orphan"), "0".repeat(40))
        .expect("write a ref in the released repository");

    // Waiting out the deadline is the assertion here: there is no event to wait *for*, so the only
    // way to know none arrived is to give one every chance to.
    std::thread::sleep(DEBOUNCE * 4);
    assert!(
        !collected(&seen).contains(&bare),
        "a released repository must not be reported: {:?}",
        collected(&seen)
    );
}

/// Write a loose ref under `plain/.git/refs/heads/`, which both `plain` and its linked worktree
/// watch.
///
/// A ref write rather than a `git` invocation: it is the smallest thing that produces a real event
/// inside the watch set, and it lands in `refs/` **only** — so a test using it fails if `refs/` is
/// not watched, where a `git commit` would also touch `index` and `logs/HEAD` and pass regardless.
fn write_ref(fixture: &fixtures::Fixture, name: &str) {
    let tip = std::fs::read_to_string(fixture.path("plain/.git/refs/heads/main"))
        .expect("the fixture committed, so main exists");
    std::fs::write(fixture.path("plain/.git/refs/heads").join(name), tip)
        .expect("write a ref inside the watched tree");
}

/// Discover the whole fixture tree, submodule included.
fn discovered(fixture: &fixtures::Fixture) -> Vec<DiscoveredRepo> {
    let opts = ScanOpts {
        descend_into_repos: true,
        ..ScanOpts::default()
    };
    let (repos, summary) = discover_roots(
        &[fixture.root().to_path_buf()],
        &opts,
        &AtomicBool::new(false),
    );
    assert!(
        summary.errors.is_empty(),
        "the fixture should discover cleanly: {:?}",
        summary.errors
    );
    repos
}

/// A watcher whose reported paths land in the returned collection.
///
/// The callback appends and returns, which is what a callback in this position has to do: anything
/// slower stalls the debouncer's thread and lets OS events pile up until they are dropped.
fn watching() -> (RepoWatcher, Arc<Mutex<Vec<PathBuf>>>) {
    let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::default();

    let recording = Arc::clone(&seen);
    let watcher = RepoWatcher::new(DEBOUNCE, move |event| {
        if let WatchEvent::Changed(paths) = event {
            lock(&recording).extend(paths);
        }
    })
    .expect("the platform's watcher backend starts");

    (watcher, seen)
}

/// Wait for `path` to be reported, up to [`DEADLINE`].
fn wait_for(seen: &Arc<Mutex<Vec<PathBuf>>>, path: &Path) -> bool {
    let started = Instant::now();
    while started.elapsed() < DEADLINE {
        if lock(seen).iter().any(|reported| reported == path) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Everything reported so far, deduplicated.
fn collected(seen: &Arc<Mutex<Vec<PathBuf>>>) -> HashSet<PathBuf> {
    lock(seen).iter().cloned().collect()
}

/// The recursion mode registered for `path`, or `None` when it is not in the set.
fn mode_of(watches: &[(PathBuf, RecursiveMode)], path: &Path) -> Option<RecursiveMode> {
    watches
        .iter()
        .find(|(watched, _)| watched == path)
        .map(|(_, mode)| *mode)
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

/// Lock through a poison rather than failing a test because another thread already did.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
