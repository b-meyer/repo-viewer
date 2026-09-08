//! Generate a synthetic tree of repositories, for the timing numbers a real tree cannot give.
//!
//!     cargo run --release --example synth -- <dir> [count] [depth]
//!
//! Two things the machine's own repositories cannot supply. First, scale: the largest real tree
//! here is a few dozen repositories, and the design is meant to hold at hundreds. Second, and more
//! importantly, a **commit-graph pair** — almost no real repository has
//! `objects/info/commit-graph`, and writing one into a repository someone owns is not this tool's
//! business. Every repository here is generated, so writing to them is free of that problem:
//!
//!     cargo run --release --example synth -- /tmp/synth 120
//!     cargo run --release --example scan  -- /tmp/synth          # without commit-graphs
//!     # then, in each generated repo:  git commit-graph write --reachable
//!     cargo run --release --example scan  -- /tmp/synth          # with commit-graphs
//!
//! Each repository is left genuinely diverged from its upstream, because a synthetic tree where
//! every repository is in sync would measure only the equal-tips short-circuit and none of the
//! revision walk the commit-graph actually accelerates.
//!
//! Put the tree **outside** the workspace. A few hundred repositories under `target/` get indexed
//! by rust-analyzer and removed by `cargo clean`.

use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// Repositories to generate when no count is given.
const DEFAULT_COUNT: usize = 120;

/// Commits per generated repository when no depth is given.
///
/// The walk cost is what the commit-graph affects, so the history has to be deep enough for the
/// difference to clear the noise floor.
const DEFAULT_DEPTH: usize = 200;

/// How far each clone is moved off its upstream, as a fraction of `depth`.
const DIVERGE_FRACTION: usize = 2;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(target) = args.next().map(PathBuf::from) else {
        eprintln!("usage: synth <dir> [count] [depth]");
        return ExitCode::FAILURE;
    };
    let count = parse_or(args.next(), DEFAULT_COUNT);
    let depth = parse_or(args.next(), DEFAULT_DEPTH);

    if target.exists() {
        eprintln!(
            "{} already exists — pick a path that does not, so an existing tree is never clobbered",
            target.display()
        );
        return ExitCode::FAILURE;
    }
    if let Err(err) = std::fs::create_dir_all(&target) {
        eprintln!("cannot create {}: {err}", target.display());
        return ExitCode::FAILURE;
    }

    // One seed repository with real history, cloned rather than rebuilt. `git clone --local`
    // hardlinks its object store, so a hundred copies cost almost nothing on disk and one
    // commit-writing pass instead of a hundred.
    println!("seeding {depth} commits...");
    let seed = target.join("seed.git");
    build_seed(&seed, depth);

    println!("cloning {count} repositories...");
    let url = seed.to_string_lossy().replace('\\', "/");
    let behind = depth / DIVERGE_FRACTION;
    for index in 0..count {
        let name = format!("repo-{index:04}");
        git(&target, &["clone", "--local", "--quiet", &url, &name]);

        // Rewind the tracking ref rather than committing locally: one process instead of many,
        // and it leaves every clone genuinely `behind` commits ahead of its upstream.
        let repo = target.join(&name);
        let base = format!("HEAD~{behind}");
        git(&repo, &["update-ref", "refs/remotes/origin/main", &base]);
    }

    println!();
    println!("generated {count} repositories in {}", target.display());
    println!("each {behind} commits ahead of its upstream, over {depth} commits of history");
    println!();
    println!("measure without commit-graphs:");
    println!(
        "  cargo run --release --example scan -- {}",
        target.display()
    );
    println!();
    println!("then write them and measure again:");
    println!(
        "  for g in {}/repo-*; do git -C \"$g\" commit-graph write --reachable; done",
        target.display()
    );

    ExitCode::SUCCESS
}

/// Build the one repository every clone comes from.
fn build_seed(seed: &Path, depth: usize) {
    std::fs::create_dir_all(seed).expect("create the seed directory");
    git(seed, &["init", "--quiet"]);

    // Empty commits: the history depth is what the revision walk pays for, and file content is
    // not part of Tier 0 at all.
    for index in 0..depth {
        let message = format!("commit {index}");
        git(
            seed,
            &["commit", "--allow-empty", "--quiet", "-m", &message],
        );
    }

    // Belt and braces over the auto-maintenance config in `git`: if a graph exists at this point
    // it would be hardlinked into every clone, and the baseline measurement would be a lie.
    let info = seed.join(".git").join("objects").join("info");
    let _ = std::fs::remove_file(info.join("commit-graph"));
    let _ = std::fs::remove_dir_all(info.join("commit-graphs"));
    assert!(
        !info.join("commit-graph").exists() && !info.join("commit-graphs").exists(),
        "the seed still has a commit-graph, so the without-graph baseline would be invalid"
    );
}

/// Run `git` in `cwd`, panicking with stderr if it fails.
///
/// Carries the test fixtures' two isolation requirements — `protocol.file.allow=always`, without
/// which cloning a local path is refused, and an empty global and system config so the developer's
/// own settings cannot change the generated tree — plus one specific to measuring.
///
/// **Auto-maintenance is off.** `git commit` runs `git maintenance run --auto`, and the
/// commit-graph task's own threshold (`maintenance.commit-graph.auto`) is 100 commits not yet in
/// the graph. A seed deep enough for the walk to be worth measuring therefore writes itself a
/// commit-graph chain unasked, `git clone --local` hardlinks it into every clone, and the
/// "without a commit-graph" baseline silently becomes a second "with" measurement.
fn git(cwd: &Path, args: &[&str]) {
    let missing_config = cwd.join("no-such-gitconfig");
    let output = Command::new("git")
        .current_dir(cwd)
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "user.name=repo-viewer synth",
            "-c",
            "user.email=synth@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
            // See the note above: without these the baseline measures the wrong thing.
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "fetch.writeCommitGraph=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &missing_config)
        .env("GIT_CONFIG_SYSTEM", &missing_config)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git — this example needs it on PATH");

    assert!(
        output.status.success(),
        "git {args:?} in {} failed with {}\n{}",
        cwd.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Parse an optional positional argument, falling back to `default`.
fn parse_or(arg: Option<String>, default: usize) -> usize {
    arg.and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
