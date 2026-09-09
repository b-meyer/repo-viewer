//! Reading one row on demand: the lazy tier, and an explicit refresh.
//!
//! Everything else in this crate drives a whole tree. These two commands drive a single
//! repository, which is what makes Tier 2 affordable at all: the full index-to-worktree diff costs
//! roughly what Tier 1 costs without its early exit, so it runs when a user asks for one row
//! rather than for all of them.
//!
//! # Two validation domains, not one
//!
//! Both commands take a path from the webview, which is untrusted regardless of where the webview
//! says it came from. They check it against **different** maps, and the difference is deliberate:
//!
//! - [`full_status`] requires a key of the canonical row map. Tier 2 fills fields on an existing
//!   row, so without one there is nothing to merge into and nothing to return.
//! - [`refresh_repo`] requires a key of the *discovered* map, which is a superset — it holds
//!   repositories whose HEAD could not be read and which therefore have no row. That is exactly
//!   the case worth retrying, and `merge_tier0` already inserts where there was nothing, so a
//!   repository that failed at scan time becomes recoverable without rescanning the tree.
//!
//! # Cancellation is a known limitation here
//!
//! Each call makes its own interrupt flag and nothing ever flips it, so collapsing the drawer
//! part-way through a large repository's read does not stop the read. A per-repository interrupt
//! registry would fix it and has no second caller until the watcher arrives, so the flag exists
//! to satisfy `gix`'s signature and nothing more.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, anyhow};
use repo_scan::{DiscoveredRepo, RepoEvent, RepoStatus, Tier, read_tier0, read_tier1, read_tier2};
use tauri::State;

use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

/// Read Tier 2 for one repository and return the merged row.
///
/// Returns the **merged row**, not a Tier 2 payload of its own: `counts` and `submodules` are
/// fields of `RepoStatus`, Rust owns the canonical copy of it, and a second shape carrying the same
/// two values would be a second source of truth for them. The caller mirrors what comes back the
/// same way it mirrors a scan batch, which is what makes an expanded-then-collapsed row keep its
/// counts with no cache of its own.
///
/// A Tier 2 failure comes back as `Err` rather than landing on `RepoStatus.error`. The row's one
/// error slot has two writers already, and this command runs once per expand — appending would
/// stack a message per expand, and replacing would erase the reason an earlier tier's field is
/// missing. There is a caller waiting on a return value, so the failure goes to it.
#[tauri::command]
pub async fn full_status(
    state: State<'_, Arc<AppState>>,
    path: PathBuf,
) -> CommandResult<RepoStatus> {
    if !state.has_repo(&path) {
        return Err(not_a_known_repository(&path));
    }
    let found = state
        .discovered(&path)
        .ok_or_else(|| not_a_known_repository(&path))?;

    let handle = Arc::clone(&state);
    spawn(move || read_full(&handle, &found)).await
}

/// Read Tier 2 for `found` and merge it into the row.
///
/// A partial failure — one of the two halves — is reported rather than swallowed, so the drawer can
/// say which. The merged row still carries whichever half succeeded, because the merge happened
/// before this returned.
fn read_full(state: &AppState, found: &DiscoveredRepo) -> anyhow::Result<RepoStatus> {
    let flag = Arc::new(AtomicBool::new(false));

    let Some(tier2) = read_tier2(found, &flag).context("could not read the file counts")? else {
        // A bare repository has no worktree and no `.gitmodules`, so neither question applies. The
        // row comes back untouched and the UI reads `n/a` off its `kind` — the same thing it
        // already does for Tier 1's `dirty`.
        return state
            .row(&found.path)
            .ok_or_else(|| anyhow!("`{}` no longer has a row", found.path.display()));
    };

    let cause = tier2.error.clone();
    let merged = state
        .merge_tier2(tier2)
        .ok_or_else(|| anyhow!("`{}` no longer has a row", found.path.display()))?;

    match cause {
        Some(cause) => Err(anyhow!("{cause}")),
        None => Ok(merged),
    }
}

/// Re-read one repository up to and including `tier`, and return the merged row.
///
/// Cumulative, because the tiers are not independent: Tier 1's `dirty` describes a worktree
/// relative to the `head` Tier 0 reads, so refreshing the one without the other would pair a fresh
/// flag with a stale ref. `Tier::Two` therefore means all three.
///
/// Pushes the merged row on the session channel **as well as** returning it. The return value
/// answers this invocation; the push is what makes this the one code path a watcher, a poll, and a
/// focus refresh can all reuse — none of those has an invocation to answer.
#[tauri::command]
pub async fn refresh_repo(
    state: State<'_, Arc<AppState>>,
    path: PathBuf,
    tier: Tier,
) -> CommandResult<RepoStatus> {
    // The discovered map, not the row map: a repository whose HEAD could not be read has no row,
    // and retrying it is precisely what this command is for.
    let found = state
        .discovered(&path)
        .ok_or_else(|| not_a_known_repository(&path))?;

    let handle = Arc::clone(&state);
    let merged = spawn(move || refresh(&handle, &found, tier)).await?;

    // A row change untied to any scan, which is what the session channel carries.
    state.push(RepoEvent::Updated {
        repos: vec![merged.clone()],
    });
    Ok(merged)
}

/// Re-read `found` up to `tier`, merging each tier as it completes.
///
/// Each tier is merged separately rather than assembled and merged once, so this shares the
/// pipeline's merge functions exactly — and so a Tier 1 failure still leaves Tier 0's fresh values
/// in the map.
fn refresh(state: &AppState, found: &DiscoveredRepo, tier: Tier) -> anyhow::Result<RepoStatus> {
    let flag = Arc::new(AtomicBool::new(false));

    // Tier 0 always runs: it is the tier that produces the row at all, and the only one whose
    // failure means there is no honest row to return.
    let row = read_tier0(found).context("could not read the repository's refs")?;
    let mut merged = state
        .merge_tier0_batch(vec![row])
        .pop()
        .ok_or_else(|| anyhow!("`{}` was read but produced no row", found.path.display()))?;

    if tier >= Tier::One
        && let Some(dirty) = read_tier1(found, &flag).context("could not read the worktree")?
    {
        merged = state.merge_tier1_batch(vec![dirty]).pop().unwrap_or(merged);
    }

    if tier >= Tier::Two
        && let Some(counts) = read_tier2(found, &flag).context("could not read the file counts")?
    {
        merged = state.merge_tier2(counts).unwrap_or(merged);
    }

    Ok(merged)
}

/// Run a blocking read off the event-loop thread.
///
/// Every read here touches the filesystem, and Tier 2 can take seconds on a large repository.
/// Inline on the runtime's thread that would freeze the window.
async fn spawn<T, F>(read: F) -> Result<T, CommandError>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    Ok(tauri::async_runtime::spawn_blocking(read)
        .await
        .context("the read did not finish")??)
}

/// The refusal for a path that is not a key of the map the caller checked.
fn not_a_known_repository(path: &std::path::Path) -> CommandError {
    anyhow!("`{}` is not a known repository", path.display()).into()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use repo_scan::{RepoKind, ScanOpts, discover_roots};

    use super::*;

    /// This repository's own root. `repo-scan`'s fixtures live in its `tests/support/` and are not
    /// reachable from here, so the tests that need a real repository use this one — the same choice
    /// `pipeline.rs`'s tests make.
    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri has a parent")
            .to_path_buf()
    }

    /// A state holding one real repository, discovered and read through Tier 0.
    ///
    /// `None` when this checkout is not a git repository, which is the one condition under which
    /// these tests have nothing to run against.
    fn seeded() -> Option<(Arc<AppState>, DiscoveredRepo)> {
        let root = workspace_root();
        if !root.join(".git").exists() {
            return None;
        }

        let opts = ScanOpts {
            max_depth: Some(1),
            ..ScanOpts::default()
        };
        let (found, _) = discover_roots(&[root], &opts, &AtomicBool::new(false));
        let repo = found
            .iter()
            .find(|repo| repo.kind == RepoKind::Normal)?
            .clone();

        let state = Arc::new(AppState::default());
        state.record_found(std::slice::from_ref(&repo));
        let row = read_tier0(&repo).expect("this repository reads");
        state.merge_tier0_batch(vec![row]);

        Some((state, repo))
    }

    /// The deliverable, at the layer that produces it: Tier 2 lands on the canonical row, so the
    /// counts survive in Rust's map rather than in whatever asked for them.
    #[test]
    fn full_status_merges_the_counts_onto_the_canonical_row() {
        let Some((state, repo)) = seeded() else {
            return;
        };

        assert_eq!(
            state.row(&repo.path).expect("a row exists").counts,
            None,
            "Tier 2 has not run yet"
        );

        let returned = read_full(&state, &repo).expect("Tier 2 reads this repository");

        assert!(returned.counts.is_some(), "the returned row carries counts");
        assert_eq!(
            state.row(&repo.path).expect("a row exists").counts,
            returned.counts,
            "and the map holds exactly what was returned"
        );
        assert!(
            returned.submodules.is_some(),
            "an empty submodule list is still a read one"
        );
    }

    /// Tier ownership at the command layer: a Tier 0 refresh must not clear the counts a Tier 2
    /// read put there. This is the case a wholesale row replacement gets wrong.
    #[test]
    fn a_tier0_refresh_keeps_the_counts_tier2_produced() {
        let Some((state, repo)) = seeded() else {
            return;
        };
        read_full(&state, &repo).expect("Tier 2 reads this repository");

        let refreshed = refresh(&state, &repo, Tier::Zero).expect("Tier 0 re-reads");

        assert!(
            refreshed.counts.is_some(),
            "a Tier 0 refresh does not own `counts` and must leave it alone"
        );
        assert_eq!(
            refreshed.dirty, None,
            "and it did not run Tier 1, so the worktree is still unknown"
        );
    }

    /// `Tier` is cumulative: asking for Tier 2 reads all three, because a fresh `dirty` beside a
    /// stale `head` would describe two different moments.
    #[test]
    fn refreshing_at_tier_two_fills_in_every_tier() {
        let Some((state, repo)) = seeded() else {
            return;
        };

        let refreshed = refresh(&state, &repo, Tier::Two).expect("all three tiers read");

        assert!(refreshed.dirty.is_some(), "Tier 1 ran");
        assert!(refreshed.counts.is_some(), "and so did Tier 2");
        assert!(refreshed.submodules.is_some());
    }

    /// Tier 1 is not run for `Tier::Zero`, so its fields stay unknown rather than becoming stale.
    #[test]
    fn refreshing_at_tier_one_stops_before_tier_two() {
        let Some((state, repo)) = seeded() else {
            return;
        };

        let refreshed = refresh(&state, &repo, Tier::One).expect("two tiers read");

        assert!(refreshed.dirty.is_some(), "Tier 1 ran");
        assert_eq!(refreshed.counts, None, "Tier 2 did not");
    }

    /// A path the app has never heard of is refused rather than read. The webview is not trusted to
    /// supply one, whatever it claims about where it came from.
    #[test]
    fn an_unknown_path_is_refused_by_both_domains() {
        let state = Arc::new(AppState::default());
        let stranger = Path::new("C:/nowhere/at/all");

        assert!(!state.has_repo(stranger));
        assert!(state.discovered(stranger).is_none());
    }

    /// A repository Tier 0 could not read has no row and so cannot serve `full_status` — but it is
    /// in the discovered map, which is what lets `refresh_repo` retry it. The two domains differing
    /// is the point.
    #[test]
    fn a_row_less_repository_is_refreshable_but_not_expandable() {
        let Some((_, repo)) = seeded() else {
            return;
        };
        let state = Arc::new(AppState::default());
        state.record_found(std::slice::from_ref(&repo));

        assert!(
            !state.has_repo(&repo.path),
            "no Tier 0 row was ever merged, so `full_status` has nothing to fill"
        );
        assert!(
            state.discovered(&repo.path).is_some(),
            "but a refresh can produce one"
        );

        let refreshed = refresh(&state, &repo, Tier::Zero).expect("Tier 0 creates the row");
        assert!(
            state.has_repo(&repo.path),
            "and the retry inserted it where there was none"
        );
        assert_eq!(refreshed.path, repo.path);
    }
}
