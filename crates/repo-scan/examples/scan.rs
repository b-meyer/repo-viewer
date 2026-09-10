//! Run the engine without the GUI.
//!
//!     cargo run --release --example scan -- C:/Working
//!     cargo run --release --example scan -- C:/Working --rows
//!     cargo run --release --example scan -- C:/Working --tier2
//!     cargo run --release --example scan -- C:/Working --watch
//!
//! This exists to produce the timing number the whole design defends, to debug one repository
//! without a webview in the way, and to check behaviour in a CI container. Cargo compiles
//! `examples/` during `cargo test`, so it cannot rot.
//!
//! Timing is `std::time::Instant`, deliberately — not `criterion`. Criterion's repeated-sampling
//! model is the wrong shape for a whole-tree scan: 100 samples of a multi-second operation is a
//! five-minute run fought with `sample_size`. Reach for it later on micro-level pieces
//! (`ahead_behind` on one repo, the prune predicate) if they profile hot.

use std::{
    env,
    path::PathBuf,
    process::ExitCode,
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use repo_scan::{
    DiscoveredRepo, Head, RepoKind, RepoStatus, ScanOpts, Tier1, discover_roots, read_tier0_all,
    read_tier1_all_with, read_tier2,
};

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let Some(root) = args.next().map(PathBuf::from) else {
        eprintln!("usage: scan <path> [--rows] [--tier2] [--watch]");
        return ExitCode::FAILURE;
    };
    let flags: Vec<_> = args.collect();
    let rows = flags.iter().any(|arg| arg == "--rows");
    // Off by default, and one repository at a time even when asked for: Tier 2 is the tier the
    // whole design keeps off the scan path, so an example that ran it over a tree by default would
    // be measuring something the app never does.
    let tier2 = flags.iter().any(|arg| arg == "--tier2");
    let watch = flags.iter().any(|arg| arg == "--watch");

    if !root.is_dir() {
        eprintln!("not a directory: {}", root.display());
        return ExitCode::FAILURE;
    }

    let (repos, summary) = discover_roots(
        std::slice::from_ref(&root),
        &ScanOpts::default(),
        &AtomicBool::new(false),
    );

    println!("root:      {}", root.display());
    println!("repos:     {}", summary.repos_found);
    println!("visited:   {} directories", summary.dirs_visited);
    println!("pruned:    {} directories", summary.dirs_pruned);
    println!("discovery: {} ms", summary.elapsed_ms);
    print_kinds(&repos);

    // Never fatal, but never silent either: a pruned or unreadable directory may have held a
    // repository the user is looking for.
    print_errors("discovery", &summary.errors);

    println!();
    let graphs = repos.iter().filter(|repo| has_commit_graph(repo)).count();
    let (statuses, tier0) = read_tier0_all(&repos, &AtomicBool::new(false));

    // The commit-graph population is printed beside the timing because it is the single biggest
    // influence on the ahead/behind walk, and a number recorded without it is uninterpretable.
    println!(
        "tier 0:    {} ms over {} repos",
        tier0.elapsed_ms, tier0.repos_read
    );
    println!(
        "  graph:     {graphs} of {} repos have a commit-graph",
        repos.len()
    );
    print_tier0_stats(&statuses);
    print_errors("tier 0", &tier0.errors);

    println!();
    let collected = std::sync::Mutex::new(Vec::new());
    let tier1 = read_tier1_all_with(
        &repos,
        &std::sync::Arc::new(AtomicBool::new(false)),
        |row| {
            collected.lock().expect("not poisoned").push(row);
        },
    );
    let dirty_rows: Vec<Tier1> = collected.into_inner().expect("not poisoned");

    println!(
        "tier 1:    {} ms over {} repos ({} bare, skipped)",
        tier1.elapsed_ms, tier1.repos_read, tier1.bare_skipped
    );
    println!(
        "  dirty:     {} of {} worktrees",
        dirty_rows.iter().filter(|row| row.dirty).count(),
        dirty_rows.len()
    );
    let conflicted: u32 = dirty_rows.iter().map(|row| row.conflicted).sum();
    println!("  conflicts: {conflicted} files across the tree");
    print_errors("tier 1", &tier1.errors);

    if tier2 {
        println!();
        print_tier2(&repos);
    }

    if watch {
        println!();
        print_watch(&repos);
    }

    if rows {
        println!();
        print_rows(&statuses);
    }

    ExitCode::SUCCESS
}

/// Register the whole tree's watch set and report what it cost.
///
/// The number that matters is **paths**, not repositories: each one is an OS handle, and on Linux
/// each one counts against `fs.inotify.max_user_watches`. It is lower than three per repository,
/// because linked worktrees share their common directory with the repository they came from and a
/// repository with no commits has no `logs/HEAD` to watch.
///
/// The watcher is dropped at the end of this function, which stops it — so this measures
/// registration and nothing else. Watching a tree for events has no meaning without an app to push
/// them to.
fn print_watch(repos: &[DiscoveredRepo]) {
    let started = std::time::Instant::now();
    let mut watcher = match repo_scan::RepoWatcher::new(Duration::from_millis(400), |_| {}) {
        Ok(watcher) => watcher,
        Err(error) => {
            println!("watch:     could not start a watcher: {error}");
            return;
        }
    };
    let built = started.elapsed();

    let registering = std::time::Instant::now();
    let failures = watcher.sync(repos);
    let registered = registering.elapsed();

    println!(
        "watch:     {} paths over {} repos",
        watcher.watched_paths(),
        watcher.watched_repos()
    );
    println!(
        "  build:     {} ms to create the debouncer",
        built.as_millis()
    );
    println!(
        "  register:  {} ms, {:.2} ms per repo",
        registered.as_millis(),
        registered.as_secs_f64() * 1000.0 / repos.len().max(1) as f64
    );
    println!(
        "  per repo:  {:.2} paths",
        watcher.watched_paths() as f64 / repos.len().max(1) as f64
    );
    print_errors("watch", &failures);

    // The release half of the diff, measured because it runs on every completed scan.
    let releasing = std::time::Instant::now();
    watcher.sync(&[]);
    println!(
        "  release:   {} ms to unwatch everything",
        releasing.elapsed().as_millis()
    );
}

/// Time Tier 2 one repository at a time, and report the spread.
///
/// **Per repository, never as a pass.** Tier 2 has no fan-out for exactly this reason: it runs when
/// a user expands one row, so the number that matters is what that one expand costs — a whole-tree
/// total would describe an operation the app never performs and invite someone to put it in the
/// pipeline.
///
/// The slowest repository is called out because it is the one a user would notice, and a mean over
/// a tree of mostly-clean checkouts hides it completely.
fn print_tier2(repos: &[DiscoveredRepo]) {
    let flag = std::sync::Arc::new(AtomicBool::new(false));
    let mut timings: Vec<(std::time::Duration, &DiscoveredRepo)> = Vec::new();
    let mut skipped = 0_usize;
    let mut failed = 0_usize;
    let mut submodules = 0_usize;

    for repo in repos {
        let started = std::time::Instant::now();
        match read_tier2(repo, &flag) {
            Ok(Some(read)) => {
                timings.push((started.elapsed(), repo));
                submodules += read.submodules.map(|list| list.len()).unwrap_or(0);
                if read.error.is_some() {
                    failed += 1;
                }
            }
            // Bare: no worktree, so the question does not apply.
            Ok(None) => skipped += 1,
            Err(err) => {
                failed += 1;
                println!("  {}: {err}", repo.path.display());
            }
        }
    }

    let total: std::time::Duration = timings.iter().map(|(elapsed, _)| *elapsed).sum();
    println!(
        "tier 2:    {} ms over {} repos ({skipped} bare, skipped) — per repo, not a pass",
        total.as_millis(),
        timings.len()
    );
    if !timings.is_empty() {
        println!(
            "  mean:      {:.1} ms per expand",
            total.as_secs_f64() * 1000.0 / timings.len() as f64
        );
    }
    if let Some((slowest, repo)) = timings.iter().max_by_key(|(elapsed, _)| *elapsed) {
        println!("  slowest:   {} ms — {}", slowest.as_millis(), repo.name);
    }
    println!("  submodules: {submodules} recorded across the tree");
    if failed > 0 {
        println!("  degraded:  {failed} repositories lost at least one half");
    }
}

/// Print the discovered repositories by kind.
fn print_kinds(repos: &[DiscoveredRepo]) {
    let mut kinds = [0_usize; 4];
    for repo in repos {
        let slot = match repo.kind {
            RepoKind::Normal => 0,
            RepoKind::Bare => 1,
            RepoKind::LinkedWorktree => 2,
            RepoKind::Submodule => 3,
        };
        kinds[slot] += 1;
    }
    println!(
        "  normal {}, bare {}, worktree {}, submodule {}",
        kinds[0], kinds[1], kinds[2], kinds[3]
    );
}

/// Print what Tier 0 actually found, not just how long it took.
///
/// The slowest single repository is called out because one pathological repository is exactly what
/// a total hides, and it is the thing worth opening next.
fn print_tier0_stats(statuses: &[RepoStatus]) {
    let tracked = statuses
        .iter()
        .filter(|status| status.upstream.is_some())
        .count();
    let unpushed = statuses
        .iter()
        .filter(|status| status.ahead.is_some_and(|ahead| ahead > 0))
        .count();
    let stale = statuses
        .iter()
        .filter(|status| status.behind.is_some_and(|behind| behind > 0))
        .count();
    let unknown = statuses
        .iter()
        .filter(|status| status.upstream.is_some() && status.ahead.is_none())
        .count();
    let degraded = statuses
        .iter()
        .filter(|status| status.error.is_some())
        .count();

    println!(
        "  upstream:  {tracked} tracked, {} without, {unknown} with no tracking ref",
        statuses.len() - tracked
    );
    println!("  unpushed:  {unpushed} ahead, {stale} behind");

    let stashes: u32 = statuses.iter().map(|status| status.stash_count).sum();
    println!("  stashes:   {stashes} across the tree");

    let oldest = statuses
        .iter()
        .filter_map(|status| status.last_fetched_ms)
        .min();
    match oldest {
        Some(fetched) => println!("  fetched:   oldest is {} days ago", days_since(fetched)),
        None => println!("  fetched:   none of these have ever been fetched"),
    }

    if degraded > 0 {
        println!("  degraded:  {degraded} rows carry a field-level error");
    }
}

/// One line per row, for debugging a single repository without a webview in the way.
fn print_rows(statuses: &[RepoStatus]) {
    for status in statuses {
        let head = match &status.head {
            Head::Branch { name } => name.clone(),
            Head::Detached { id } => format!("detached@{}", &id[..7.min(id.len())]),
            Head::Unborn => "unborn".to_string(),
        };
        let counts = match (status.ahead, status.behind) {
            (Some(ahead), Some(behind)) => format!("+{ahead}/-{behind}"),
            _ => "?/?".to_string(),
        };
        println!(
            "  {:<28} {:<22} {:<10} stash {:<3} {:?}{}",
            truncate(&status.name, 28),
            truncate(&head, 22),
            counts,
            status.stash_count,
            status.state,
            status
                .error
                .as_deref()
                .map(|err| format!("  !! {err}"))
                .unwrap_or_default(),
        );
    }
}

/// Print a list of non-fatal failures, if there are any.
fn print_errors(stage: &str, errors: &[repo_scan::ScanError]) {
    if errors.is_empty() {
        return;
    }
    println!("\n{stage} errors ({}):", errors.len());
    for error in errors {
        println!("  {}: {}", error.path.display(), error.message);
    }
}

/// Whether a repository has a commit-graph on disk, in either layout.
fn has_commit_graph(repo: &DiscoveredRepo) -> bool {
    let info = repo.git_dir.join("objects").join("info");
    info.join("commit-graph").is_file() || info.join("commit-graphs").is_dir()
}

/// Whole days between `epoch_ms` and now.
fn days_since(epoch_ms: u64) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0);
    now.saturating_sub(epoch_ms) / (1000 * 60 * 60 * 24)
}

/// Clip `text` to `width` characters so the row table stays aligned.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width - 1).chain(['…']).collect()
}
