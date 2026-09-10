//! `git fetch`, through the CLI.
//!
//! The one place this engine writes anything. Everything else reads.
//!
//! # Why the CLI and not `gix`
//!
//! Fetch is where credential helpers, SSH config (`~/.ssh/config`, agents, jump hosts), corporate
//! proxies and custom transports matter. `gix` is built with default features here — no network,
//! no TLS, and therefore no C dependency, which is §10.3's whole point — and reimplementing
//! credential discovery on top of it would be both a large surface and a worse one. Delegating to
//! the `git` the user already has means Git Credential Manager, Keychain, and whatever their
//! `.ssh/config` says all work without this app knowing they exist. It is also why no `keyring`
//! dependency is needed.
//!
//! # The pool is scoped threads, not rayon
//!
//! rayon's global pool is shared with Tier 0 and Tier 1. Parking four workers on 60-second
//! subprocesses would halve the throughput of the tier that dominates a scan, and there is no way
//! to bound in-flight work to four across a 300-item `par_iter` except chunking, which serialises
//! within a chunk — one slow repository would block its neighbours. So [`fetch_all_with`] uses
//! `std::thread::scope`: the threads are bounded by the concurrency cap, joined before the call
//! returns, and cannot outlive their borrows. There is still no `mpsc` here — the callback is the
//! channel, exactly as in [`crate::read_tier0_all_with`].
//!
//! # Classification is advisory, and only half of it is prose
//!
//! Six of the nine [`FetchStatus`] values are decided structurally: from a pre-flight config read,
//! from our own bookkeeping, or from an exit status. Getting `NoRemote` in particular out of the
//! prose bucket matters, because it is the most common non-success in a real tree — every scratch
//! repository a developer ever `git init`ed — and git's wording for it has changed more than once.
//!
//! Only `Auth` and `Network` are decided by matching git's own words, and there is no error kind
//! to match instead, unlike `notify`'s. So the seeds below are a list to add to when a real one is
//! observed, not a closed set, and [`FetchOutcome::detail`] always carries git's own message
//! whatever the classification said. A misclassification costs an icon; it never costs the user
//! the truth.
//!
//! # Two things that would deadlock or lie if changed
//!
//! **stderr is piped and read only after the child exits.** That is safe *because* of `--quiet`
//! and because git suppresses progress output when stderr is not a tty, so the output stays far
//! under the pipe buffer. Adding `--progress` for a nicer log line would reintroduce a genuine
//! deadlock — git blocks writing to a full pipe, never exits, and the deadline then kills a fetch
//! that was working. It would present as "only large repositories time out".
//!
//! **The environment is inherited whole.** Sanitising a subprocess environment is a reflex and it
//! would destroy the entire reason for using the CLI: `GIT_SSH_COMMAND`, `SSH_AUTH_SOCK`,
//! `HTTP(S)_PROXY`, `HOME` and the credential-helper configuration all arrive that way. Note the
//! deliberate contrast with `tests/support/fixtures.rs`, which points `GIT_CONFIG_GLOBAL` at a
//! file that does not exist — correct for a fixture, catastrophic here.

use std::{
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use crate::{
    exe::resolve_program,
    model::{DiscoveredRepo, FetchOutcome, FetchStatus, FetchSummary, GitInfo},
};

/// §8.2's per-process deadline.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// §8.2's "at most ~4 concurrent fetches".
pub const DEFAULT_CONCURRENCY: usize = 4;

/// How often a running child is checked against its deadline and the interrupt flag.
///
/// 25 ms adds at most 25 ms to a fetch measured in seconds, and costs a handful of sleeping
/// threads. `std` has no `Child::wait_timeout`, and the `wait-timeout` crate installs a
/// process-wide `SIGCHLD` handler to save these twelve lines — a poor trade in a process that
/// also spawns editors and terminals it deliberately does not reap.
const POLL_TICK: Duration = Duration::from_millis(25);

/// How much of git's stderr is kept on an outcome.
const DETAIL_MAX: usize = 2_000;

/// Windows' "give this process no console" creation flag.
///
/// `commands/open.rs` declares its own copy and uses it for the opposite decision: a fetch must
/// never flash a console, and a terminal launch must always get one. Same platform, same flag,
/// opposite answers — which is why there are two declarations rather than one shared helper.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How to fetch.
#[derive(Debug, Clone)]
pub struct FetchOpts {
    /// The resolved `git`.
    ///
    /// Resolved by the caller, per invocation, rather than probed once and remembered: a `PATH`
    /// can change while the app runs, so a startup probe is an affordance and this is the truth.
    pub program: PathBuf,
    /// Per-process wall-clock deadline.
    pub timeout: Duration,
    /// At most this many `git` processes alive at once.
    pub concurrency: usize,
    /// The shortest gap between two fetches of one repository, measured from `FETCH_HEAD`'s mtime.
    ///
    /// `None` disables the guard, which is what a single per-row request passes: a user clicking
    /// one repository's button twice is expressing intent, and the hazard §8.2 names is fetching
    /// three hundred repositories unprompted.
    pub min_interval: Option<Duration>,
    /// Delete remote-tracking refs whose remote branch is gone.
    pub prune: bool,
    /// Fetch every configured remote rather than only the current branch's.
    pub all_remotes: bool,
}

impl Default for FetchOpts {
    fn default() -> Self {
        Self {
            program: PathBuf::from("git"),
            timeout: DEFAULT_TIMEOUT,
            concurrency: DEFAULT_CONCURRENCY,
            min_interval: None,
            prune: true,
            all_remotes: true,
        }
    }
}

/// What a pass has to say as it runs.
///
/// Not on the wire: `src-tauri` maps it onto `FetchEvent`. One enum rather than two callback
/// parameters, so the signature keeps the shape [`crate::read_tier0_all_with`] established.
#[derive(Debug, Clone)]
pub enum FetchNotice {
    /// A process is about to be spawned for this repository.
    ///
    /// Sent from the worker thread **before** the spawn, because a consumer that suppresses a
    /// file watcher for the repository has to have done so before the first byte is written.
    Started(PathBuf),
    /// This repository has settled, whatever the outcome.
    Done(FetchOutcome),
}

/// Fetch one repository.
///
/// Never fails: every outcome is a value, exactly as a per-repository read is. There is no `Err`
/// to lose a repository to.
///
/// `should_interrupt` is checked on every poll tick, so a window close kills a child mid-transfer
/// rather than waiting out the deadline. Orphaning it instead would leave a `git` writing into a
/// repository the app no longer owns, holding a directory handle open on Windows.
pub fn fetch_one(
    repo: &DiscoveredRepo,
    opts: &FetchOpts,
    should_interrupt: &AtomicBool,
) -> FetchOutcome {
    if should_interrupt.load(Ordering::Relaxed) {
        return outcome(repo, FetchStatus::Cancelled, None, 0);
    }

    // Pre-flight, in this order, because each is cheaper than the one after it and both are
    // cheaper than a process. Answering "nothing to fetch from" here rather than by reading a
    // `fatal:` line is what keeps the most common non-success out of the prose bucket.
    //
    // `has_remote` answering `None` — the repository will not open — is deliberately not a
    // refusal: `git` is left to give the real reason in its own words rather than this guessing
    // at one.
    if has_remote(repo) == Some(false) {
        return outcome(repo, FetchStatus::NoRemote, None, 0);
    }

    if let Some(interval) = opts.min_interval
        && fetched_within(repo, interval)
    {
        return outcome(repo, FetchStatus::TooSoon, None, 0);
    }

    run(repo, opts, should_interrupt)
}

/// Fetch every repository in `repos`, at most `opts.concurrency` at a time.
///
/// `on_event` is called from worker threads, concurrently, and **must be cheap** — the same
/// contract [`crate::read_tier0_all_with`]'s `on_status` has, for the same reason: it runs on a
/// thread that has work to get back to.
///
/// Order is not guaranteed. Repositories are claimed from a shared cursor, so a slow one delays
/// nothing but itself.
pub fn fetch_all_with<F>(
    repos: &[DiscoveredRepo],
    opts: &FetchOpts,
    should_interrupt: &AtomicBool,
    on_event: F,
) -> FetchSummary
where
    F: Fn(FetchNotice) + Send + Sync,
{
    let started = Instant::now();
    if repos.is_empty() {
        return FetchSummary::default();
    }

    let cursor = AtomicUsize::new(0);
    let attempted = AtomicU32::new(0);
    let succeeded = AtomicU32::new(0);
    let failed = AtomicU32::new(0);
    let skipped = AtomicU32::new(0);

    // N threads pulling from one cursor *is* the semaphore, with less to get wrong than a permit
    // counter. Never more threads than there is work.
    let workers = opts.concurrency.clamp(1, repos.len());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(repo) = repos.get(index) else {
                        return;
                    };

                    if !should_interrupt.load(Ordering::Relaxed) {
                        on_event(FetchNotice::Started(repo.path.clone()));
                    }

                    let result = fetch_one(repo, opts, should_interrupt);
                    match result.status {
                        FetchStatus::Ok => {
                            attempted.fetch_add(1, Ordering::Relaxed);
                            succeeded.fetch_add(1, Ordering::Relaxed);
                        }
                        status if status.ran() => {
                            attempted.fetch_add(1, Ordering::Relaxed);
                            failed.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {
                            skipped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    on_event(FetchNotice::Done(result));
                }
            });
        }
    });

    FetchSummary {
        attempted: attempted.into_inner(),
        succeeded: succeeded.into_inner(),
        failed: failed.into_inner(),
        skipped: skipped.into_inner(),
        cancelled: should_interrupt.load(Ordering::Relaxed),
        elapsed_ms: elapsed_ms(started),
    }
}

/// Find `git` and ask it its version. `None` when it is absent or will not run.
///
/// Running it, rather than only resolving a path, is the point: a `git.exe` that exists on `PATH`
/// and a `git.exe` that this process may execute are different facts on a managed machine, where
/// Defender for Endpoint denies low-prevalence binaries with `os error 5`.
pub fn probe_git() -> Option<GitInfo> {
    let program = resolve_program("git");

    let mut command = Command::new(&program);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut command);

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        return None;
    }

    Some(GitInfo {
        path: program,
        version,
    })
}

/// Spawn `git`, wait for it against the deadline, and classify what came back.
fn run(repo: &DiscoveredRepo, opts: &FetchOpts, should_interrupt: &AtomicBool) -> FetchOutcome {
    let started = Instant::now();

    let mut command = Command::new(&opts.program);
    command
        .args(args_for(repo, opts))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    apply_env(&mut command);
    hide_console(&mut command);
    // Deliberately no `current_dir`: `-C` carries the target, and a child holding a directory
    // handle can stop that directory being deleted or renamed while the fetch runs.

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return outcome(
                repo,
                FetchStatus::GitMissing,
                Some(format!("`{}` could not be run", opts.program.display())),
                elapsed_ms(started),
            );
        }
        Err(error) => {
            return outcome(
                repo,
                FetchStatus::Failed,
                Some(error.to_string()),
                elapsed_ms(started),
            );
        }
    };

    let status = match wait_until(&mut child, started + opts.timeout, should_interrupt) {
        Wait::Exited(status) => status,
        Wait::Cancelled => return outcome(repo, FetchStatus::Cancelled, None, elapsed_ms(started)),
        Wait::TimedOut => {
            let detail = format!(
                "`git fetch` was still running after {}s and was stopped",
                opts.timeout.as_secs()
            );
            return outcome(
                repo,
                FetchStatus::TimedOut,
                Some(detail),
                elapsed_ms(started),
            );
        }
        Wait::Failed(message) => {
            return outcome(
                repo,
                FetchStatus::Failed,
                Some(message),
                elapsed_ms(started),
            );
        }
    };

    // Read only now that the write end is closed. See the module doc for why this cannot deadlock
    // while `--quiet` holds, and why it would if `--progress` were ever added.
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read as _;
        let _ = pipe.read_to_string(&mut stderr);
    }

    let classified = classify(status.code(), &stderr);
    outcome(repo, classified, detail(&stderr), elapsed_ms(started))
}

/// How a wait ended.
#[derive(Debug)]
enum Wait {
    /// The child exited on its own.
    Exited(std::process::ExitStatus),
    /// The interrupt flag was set and the child was killed.
    Cancelled,
    /// The deadline passed and the child was killed.
    TimedOut,
    /// The wait itself failed.
    Failed(String),
}

/// Wait for `child`, killing it at `deadline` or when interrupted.
///
/// Split out so the deadline is testable with any long-running child rather than only with a
/// `git` that has somewhere slow to fetch from. `std` has no `Child::wait_timeout`; this is the
/// twelve lines that stand in for it.
fn wait_until(
    child: &mut std::process::Child,
    deadline: Instant,
    should_interrupt: &AtomicBool,
) -> Wait {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Wait::Exited(status),
            Ok(None) => {}
            Err(error) => return Wait::Failed(error.to_string()),
        }

        let interrupted = should_interrupt.load(Ordering::Relaxed);
        if interrupted || Instant::now() >= deadline {
            // Killed rather than orphaned: an abandoned `git` keeps writing into a repository the
            // app no longer owns and holds a directory handle open on Windows. `kill` reaches
            // only the direct child — `git` spawns `git-remote-https` or `ssh` of its own — but
            // those exit when the stderr pipe they inherited closes, which happens as this
            // `Child` drops. So a test asserting "no git processes remain" immediately after a
            // kill will flake; the handle is released, not the whole tree, and not instantly.
            let _ = child.kill();
            // Reaped here rather than left to `Drop`, which does not wait: on Unix an unreaped
            // child is a zombie for the life of the process, and a bulk fetch could make three
            // hundred of them.
            let _ = child.wait();
            return if interrupted {
                Wait::Cancelled
            } else {
                Wait::TimedOut
            };
        }

        std::thread::sleep(POLL_TICK);
    }
}

/// The argument vector for one repository.
///
/// Split out and pure so the flags can be asserted as a literal — several of them are load-bearing
/// and one of them is load-bearing by its *absence*.
fn args_for(repo: &DiscoveredRepo, opts: &FetchOpts) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::with_capacity(12);

    // `git fetch` runs auto-maintenance on completion, exactly as `git commit` does — and a
    // repack is unbounded. Without these two, a bulk fetch can spend most of its wall clock
    // repacking and report the largest repositories as timed out. Config rather than
    // `--no-auto-maintenance` so it does not depend on how old the user's `git` is, and so it
    // reaches anything `git` spawns for itself.
    args.push("-c".into());
    args.push("gc.auto=0".into());
    args.push("-c".into());
    args.push("maintenance.auto=false".into());
    // The config-side equivalent of `GCM_INTERACTIVE=never`, honoured by modern credential
    // helpers and version-independent in a way the environment variable is not.
    args.push("-c".into());
    args.push("credential.interactive=false".into());

    // `repo.path` for every kind, never `git_dir` or `common_dir`. Discovery already proved this
    // is a repository with `gix::discover::is_git`, and `-C` re-runs git's own discovery from a
    // directory git classifies the same way — including a bare one, where an explicit
    // `--work-tree` would be actively wrong, and a linked worktree, where git follows the `.git`
    // file and finds the common directory itself.
    args.push("-C".into());
    args.push(repo.path.clone().into_os_string());

    args.push("fetch".into());
    // Bounds stderr, which is what makes reading it after exit safe. It does not suppress errors.
    args.push("--quiet".into());

    if opts.prune {
        // Not optional for honesty's sake: without it a deleted remote branch leaves its tracking
        // ref forever and `behind` keeps counting against a ref that no longer exists upstream.
        //
        // `--prune-tags` is deliberately **absent** and its absence is asserted below. It deletes
        // the user's own local tags, which is destructive and outside "read-only plus fetch".
        args.push("--prune".into());
    }

    // Fetch's default is `on-demand`, which recurses unpredictably, multiplies the deadline risk,
    // and trips the CVE-2022-39253 restriction for a local-path submodule.
    args.push("--no-recurse-submodules".into());

    if opts.all_remotes {
        // `last_fetched_ms` is `FETCH_HEAD`'s mtime, which is a *repository-level* fact. Fetching
        // one remote of three and then stamping the whole row as freshly fetched would overstate
        // the freshness of everything the row does not show.
        args.push("--all".into());
    }

    args
}

/// Apply the environment overlay.
///
/// An overlay, never a replacement — see the module doc. Three variables, each closing one way a
/// fetch can sit waiting for a human who is not there.
fn apply_env(command: &mut Command) {
    // Git's own username/password prompt.
    command.env("GIT_TERMINAL_PROMPT", "0");
    // Git Credential Manager pops a **GUI** window, and runs *before* git's own prompt — the
    // likeliest prompt on a Windows dev box, and the one §8.2 does not mention.
    command.env("GCM_INTERACTIVE", "never");
    // `ssh` shelling out to a graphical askpass for a key passphrase.
    command.env("SSH_ASKPASS_REQUIRE", "never");
}

/// Whether the repository has any remote configured.
///
/// `None` when the repository will not open — not a claim either way, so the caller spawns `git`
/// and lets it give the real reason rather than inventing one.
fn has_remote(repo: &DiscoveredRepo) -> Option<bool> {
    let options = gix::open::Options::default().open_path_as_is(true);
    let opened = gix::open_opts(&repo.git_dir, options).ok()?;
    Some(!opened.remote_names().is_empty())
}

/// Whether `FETCH_HEAD` was written inside `interval`.
///
/// The clock is the file on disk rather than a map in memory: it is already read by Tier 0,
/// already displayed in the table, and it survives a restart. Two consequences fall out and both
/// are wanted — a *failed* fetch does not update it, so a repository whose credentials were wrong
/// is retried the moment they are fixed; and one that has never been fetched is never too soon.
fn fetched_within(repo: &DiscoveredRepo, interval: Duration) -> bool {
    let Some(fetched_ms) = crate::status::tier0::fetch_head_ms(&repo.git_dir, &repo.common_dir)
    else {
        return false;
    };
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return false;
    };
    let now_ms = u64::try_from(now.as_millis()).unwrap_or(u64::MAX);

    now_ms.saturating_sub(fetched_ms) < u64::try_from(interval.as_millis()).unwrap_or(u64::MAX)
}

/// Classify a finished `git fetch` from its exit status and stderr.
///
/// Pure, so the taxonomy is testable against captured stderr with no network, no remote and no
/// process. Auth is checked before network because an HTTPS URL that fails authentication often
/// prints the URL too, and both shapes mention it.
fn classify(code: Option<i32>, stderr: &str) -> FetchStatus {
    if code == Some(0) {
        return FetchStatus::Ok;
    }

    let lower = stderr.to_ascii_lowercase();

    const AUTH: &[&str] = &[
        "authentication failed",
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "permission denied (publickey",
        "access denied",
        "403 forbidden",
        "authorization failed",
    ];
    const NETWORK: &[&str] = &[
        "could not resolve host",
        "could not resolve hostname",
        "connection refused",
        "connection timed out",
        "network is unreachable",
        "failed to connect to",
        "operation timed out",
        "no route to host",
        "proxy",
    ];

    if AUTH.iter().any(|needle| lower.contains(needle)) {
        return FetchStatus::Auth;
    }
    if NETWORK.iter().any(|needle| lower.contains(needle)) {
        return FetchStatus::Network;
    }

    // A non-zero exit is never success. Exit 1 in particular means "some refs failed to update",
    // which is a genuine partial failure — reporting it as `Ok` would be the
    // uncomputed-renders-as-zero bug pointed at an exit code.
    FetchStatus::Failed
}

/// Git's last words, trimmed and capped.
///
/// The **tail**, because git prints its `fatal:` line last.
fn detail(stderr: &str) -> Option<String> {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.len() <= DETAIL_MAX {
        return Some(trimmed.to_string());
    }

    // On a character boundary, so a multi-byte path in a message cannot panic this.
    let mut start = trimmed.len() - DETAIL_MAX;
    while start < trimmed.len() && !trimmed.is_char_boundary(start) {
        start += 1;
    }
    Some(format!("…{}", &trimmed[start..]))
}

/// Build one outcome.
fn outcome(
    repo: &DiscoveredRepo,
    status: FetchStatus,
    detail: Option<String>,
    elapsed_ms: u64,
) -> FetchOutcome {
    FetchOutcome {
        path: repo.path.clone(),
        status,
        detail,
        elapsed_ms,
    }
}

/// Give the child no console. A no-op off Windows, which has no such flag.
#[cfg_attr(
    not(windows),
    expect(unused_variables, reason = "there is no console flag off Windows")
)]
fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Milliseconds since `started`, saturating.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use crate::model::RepoKind;

    use super::*;

    /// A repository shaped for the argv tests. Nothing on disk.
    fn repo(kind: RepoKind, path: &str, git_dir: &str) -> DiscoveredRepo {
        DiscoveredRepo {
            path: PathBuf::from(path),
            name: "alpha".into(),
            parent: PathBuf::from("C:/work"),
            kind,
            git_dir: PathBuf::from(git_dir),
            common_dir: PathBuf::from(git_dir),
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// The whole vector, asserted literally. Every flag here is load-bearing and one of the
    /// absences is too, so a literal is the only assertion that catches an edit to any of them.
    #[test]
    fn the_argument_vector_is_exactly_this() {
        let found = repo(RepoKind::Normal, "C:/work/alpha", "C:/work/alpha/.git");

        let args = strings(&args_for(&found, &FetchOpts::default()));

        assert_eq!(
            args,
            vec![
                "-c",
                "gc.auto=0",
                "-c",
                "maintenance.auto=false",
                "-c",
                "credential.interactive=false",
                "-C",
                "C:/work/alpha",
                "fetch",
                "--quiet",
                "--prune",
                "--no-recurse-submodules",
                "--all",
            ]
        );
    }

    /// `--prune-tags` deletes the user's own tags. It is outside "read-only plus fetch", its
    /// neighbour `--prune` is required, and adding it would be a one-word edit — so its absence is
    /// asserted rather than left to a reviewer noticing.
    #[test]
    fn the_destructive_prune_neighbour_is_never_passed() {
        for prune in [true, false] {
            let found = repo(RepoKind::Normal, "C:/work/alpha", "C:/work/alpha/.git");
            let opts = FetchOpts {
                prune,
                ..FetchOpts::default()
            };

            let args = strings(&args_for(&found, &opts));

            assert!(
                !args.iter().any(|arg| arg == "--prune-tags"),
                "local tags are the user's own objects, got {args:?}"
            );
        }
    }

    /// Auto-maintenance is unbounded and would be charged to the deadline.
    #[test]
    fn auto_maintenance_is_disabled() {
        let found = repo(RepoKind::Normal, "C:/work/alpha", "C:/work/alpha/.git");

        let args = strings(&args_for(&found, &FetchOpts::default()));

        assert!(args.iter().any(|arg| arg == "gc.auto=0"));
        assert!(args.iter().any(|arg| arg == "maintenance.auto=false"));
    }

    /// `-C` takes the worktree for every kind, including a bare repository — where an explicit
    /// `--work-tree` would be wrong — and a linked worktree, where git follows the `.git` file.
    #[test]
    fn the_target_is_the_repository_path_for_every_kind() {
        let cases = [
            (RepoKind::Normal, "C:/work/alpha", "C:/work/alpha/.git"),
            (RepoKind::Bare, "C:/work/mirror.git", "C:/work/mirror.git"),
            (
                RepoKind::LinkedWorktree,
                "C:/work/wt",
                "C:/work/alpha/.git/worktrees/wt",
            ),
            (
                RepoKind::Submodule,
                "C:/work/alpha/sub",
                "C:/work/alpha/.git/modules/sub",
            ),
        ];

        for (kind, path, git_dir) in cases {
            let args = strings(&args_for(&repo(kind, path, git_dir), &FetchOpts::default()));

            let target = args
                .iter()
                .position(|arg| arg == "-C")
                .and_then(|index| args.get(index + 1))
                .expect("a -C target");
            assert_eq!(target, path, "{kind:?} must be fetched at its own path");
        }
    }

    /// The shape our own `GIT_TERMINAL_PROMPT=0` produces, and therefore the auth failure this
    /// app is most likely to see.
    #[test]
    fn a_disabled_prompt_reads_as_an_auth_failure() {
        let stderr = "fatal: could not read Username for 'https://example.com': \
                      terminal prompts disabled";

        assert_eq!(classify(Some(128), stderr), FetchStatus::Auth);
    }

    /// The common HTTPS shape.
    #[test]
    fn a_refused_credential_reads_as_an_auth_failure() {
        let stderr = "remote: Invalid username or password.\n\
                      fatal: Authentication failed for 'https://example.com/x.git/'";

        assert_eq!(classify(Some(128), stderr), FetchStatus::Auth);
    }

    /// The two transport shapes: no DNS answer, and an answer that refuses.
    #[test]
    fn transport_failures_read_as_network_failures() {
        assert_eq!(
            classify(
                Some(128),
                "fatal: unable to access 'https://x/': Could not resolve host: x"
            ),
            FetchStatus::Network
        );
        assert_eq!(
            classify(
                Some(128),
                "fatal: unable to access 'http://127.0.0.1:1/x.git/': \
                 Failed to connect to 127.0.0.1 port 1: Connection refused"
            ),
            FetchStatus::Network
        );
    }

    /// Exit 1 means "some refs were not updated" — a real partial failure. Reading a non-zero
    /// exit as success would be the uncomputed-renders-as-zero bug pointed at an exit code.
    #[test]
    fn a_non_zero_exit_is_never_success() {
        assert_eq!(
            classify(Some(1), "error: some local refs could not be updated"),
            FetchStatus::Failed
        );
        assert_eq!(classify(Some(1), ""), FetchStatus::Failed);
        assert_eq!(classify(None, ""), FetchStatus::Failed);
    }

    /// Only a zero exit is `Ok`, whatever the noise on stderr.
    #[test]
    fn a_zero_exit_is_success_even_with_output() {
        assert_eq!(
            classify(
                Some(0),
                "warning: redirecting to https://example.com/x.git/"
            ),
            FetchStatus::Ok
        );
    }

    /// An unrecognised failure is `Failed` and keeps git's words, because the classification is
    /// advisory and the message is what the user actually reads.
    #[test]
    fn an_unrecognised_failure_keeps_gits_own_words() {
        let stderr = "fatal: something nobody has seen before";

        assert_eq!(classify(Some(128), stderr), FetchStatus::Failed);
        assert_eq!(detail(stderr).as_deref(), Some(stderr));
    }

    /// git prints `fatal:` last, so a truncation that keeps the head throws away the only line
    /// worth reading.
    #[test]
    fn a_long_message_is_truncated_from_the_front() {
        let stderr = format!("{}\nfatal: the part that matters", "noise\n".repeat(2_000));

        let kept = detail(&stderr).expect("a detail");

        assert!(kept.starts_with('…'), "the head is dropped, got {kept:?}");
        assert!(
            kept.ends_with("fatal: the part that matters"),
            "the tail is kept, got {kept:?}"
        );
        assert!(
            kept.len() <= DETAIL_MAX + 4,
            "capped, got {} bytes",
            kept.len()
        );
    }

    /// Multi-byte output must not panic the truncation.
    #[test]
    fn truncation_lands_on_a_character_boundary() {
        let stderr = "é".repeat(4_000);

        let kept = detail(&stderr).expect("a detail");

        assert!(kept.len() <= DETAIL_MAX + 4);
    }

    /// Nothing on stderr is no detail, rather than an empty string that renders as a blank alert.
    #[test]
    fn silence_produces_no_detail() {
        assert_eq!(detail(""), None);
        assert_eq!(detail("   \n  "), None);
    }

    /// The four statuses where no process ran are the four that must not trigger a re-read.
    #[test]
    fn only_the_statuses_that_spawned_a_process_count_as_having_run() {
        assert!(FetchStatus::Ok.ran());
        assert!(FetchStatus::Failed.ran());
        assert!(FetchStatus::Auth.ran());
        assert!(FetchStatus::Network.ran());
        assert!(FetchStatus::TimedOut.ran());

        assert!(!FetchStatus::NoRemote.ran());
        assert!(!FetchStatus::TooSoon.ran());
        assert!(!FetchStatus::Cancelled.ran());
        assert!(!FetchStatus::GitMissing.ran());
    }

    /// An interrupt already set costs no process at all — checked with a program that could not
    /// possibly run, so a spawn would be unmistakable.
    #[test]
    fn an_interrupt_set_before_the_call_spawns_nothing() {
        let found = repo(RepoKind::Normal, "C:/work/alpha", "C:/work/alpha/.git");
        let opts = FetchOpts {
            program: PathBuf::from("definitely-not-a-real-program"),
            ..FetchOpts::default()
        };

        let result = fetch_one(&found, &opts, &AtomicBool::new(true));

        assert_eq!(result.status, FetchStatus::Cancelled);
        assert_eq!(result.elapsed_ms, 0);
    }

    /// An empty list is a summary of nothing, not a panic and not a thread.
    #[test]
    fn an_empty_pass_does_nothing() {
        let summary = fetch_all_with(&[], &FetchOpts::default(), &AtomicBool::new(false), |_| {
            unreachable!("nothing to report")
        });

        assert_eq!(summary, FetchSummary::default());
    }

    /// A child that will outlive any deadline these tests set, without needing `git` or a network.
    fn slow_child() -> std::process::Child {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_console(&mut command);
        command.spawn().expect("a slow child")
    }

    /// The deadline is what makes an ssh passphrase prompt or a proxy stall finite. Without it a
    /// fetch waits forever on a terminal that does not exist.
    #[test]
    fn a_child_that_outlives_its_deadline_is_killed() {
        let mut child = slow_child();
        let started = Instant::now();

        let waited = wait_until(
            &mut child,
            started + Duration::from_millis(200),
            &AtomicBool::new(false),
        );

        assert!(matches!(waited, Wait::TimedOut), "got {waited:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it returned at the deadline, not at the child's own pace"
        );
        // Reaped, not merely killed: an unreaped child is a zombie on Unix, and a bulk fetch
        // would make one per repository.
        assert!(
            matches!(child.try_wait(), Ok(Some(_))),
            "the child was waited for"
        );
    }

    /// Window close must stop a fetch in flight, not wait out its deadline.
    #[test]
    fn an_interrupt_kills_a_running_child_rather_than_waiting() {
        let mut child = slow_child();
        let interrupt = AtomicBool::new(true);
        let started = Instant::now();

        let waited = wait_until(&mut child, started + Duration::from_secs(600), &interrupt);

        assert!(matches!(waited, Wait::Cancelled), "got {waited:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "it did not wait out the 600-second deadline"
        );
    }

    /// The ordinary path: a child that finishes is reported with its own status, not killed.
    #[test]
    fn a_child_that_finishes_reports_its_own_status() {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "exit 3"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 3"]);
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_console(&mut command);
        let mut child = command.spawn().expect("a quick child");

        let waited = wait_until(
            &mut child,
            Instant::now() + Duration::from_secs(30),
            &AtomicBool::new(false),
        );

        match waited {
            Wait::Exited(status) => assert_eq!(status.code(), Some(3)),
            other => panic!("expected an exit, got {other:?}"),
        }
    }

    /// The wire spelling, which the frontend switches on.
    #[test]
    fn statuses_cross_the_wire_in_camel_case() {
        let json = |status: FetchStatus| serde_json::to_string(&status).expect("serialised");

        assert_eq!(json(FetchStatus::NoRemote), "\"noRemote\"");
        assert_eq!(json(FetchStatus::TimedOut), "\"timedOut\"");
        assert_eq!(json(FetchStatus::GitMissing), "\"gitMissing\"");
    }
}
