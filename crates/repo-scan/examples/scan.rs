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

use std::{env, path::PathBuf, process::ExitCode, time::Instant};

fn main() -> ExitCode {
    let Some(root) = env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: scan <path>");
        return ExitCode::FAILURE;
    };

    if !root.is_dir() {
        eprintln!("not a directory: {}", root.display());
        return ExitCode::FAILURE;
    }

    let started = Instant::now();
    // Discovery and the tiered reads land in Phase 1 and Phase 2. Until then this reports the
    // shape of the run rather than its results, so the harness itself stays compiled and honest.
    let repos: Vec<repo_scan::RepoStatus> = Vec::new();
    let elapsed = started.elapsed();

    println!("root:    {}", root.display());
    println!("repos:   {}", repos.len());
    println!("elapsed: {elapsed:.3?}");
    ExitCode::SUCCESS
}
