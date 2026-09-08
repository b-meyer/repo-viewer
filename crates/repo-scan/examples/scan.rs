//! Run the engine without the GUI.
//!
//!     cargo run --release --example scan -- C:/Working/Source
//!
//! This exists to produce the timing number the whole design defends, to debug one repository
//! without a webview in the way, and to check behaviour in a CI container. Cargo compiles
//! `examples/` during `cargo test`, so it cannot rot.
//!
//! Timing is `std::time::Instant`, deliberately — not `criterion`. Criterion's repeated-sampling
//! model is the wrong shape for a whole-tree scan: 100 samples of a multi-second operation is a
//! five-minute run fought with `sample_size`. Reach for it later on micro-level pieces
//! (`ahead_behind` on one repo, the prune predicate) if they profile hot.

use std::{env, path::PathBuf, process::ExitCode};

use repo_scan::{RepoKind, ScanOpts, discover_roots};

fn main() -> ExitCode {
    let Some(root) = env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: scan <path>");
        return ExitCode::FAILURE;
    };

    if !root.is_dir() {
        eprintln!("not a directory: {}", root.display());
        return ExitCode::FAILURE;
    }

    // Tier 0 lands in Phase 2 and reports its own timing; this is the discovery number alone.
    let (repos, summary) = discover_roots(std::slice::from_ref(&root), &ScanOpts::default());

    println!("root:      {}", root.display());
    println!("repos:     {}", summary.repos_found);
    println!("visited:   {} directories", summary.dirs_visited);
    println!("pruned:    {} directories", summary.dirs_pruned);
    println!("elapsed:   {} ms", summary.elapsed_ms);

    let mut kinds = [0_usize; 4];
    for repo in &repos {
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

    // Never fatal, but never silent either: a pruned or unreadable directory may have held a
    // repository the user is looking for.
    if !summary.errors.is_empty() {
        println!("\nerrors ({}):", summary.errors.len());
        for error in &summary.errors {
            println!("  {}: {}", error.path.display(), error.message);
        }
    }

    ExitCode::SUCCESS
}
