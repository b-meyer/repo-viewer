//! Counting commits on either side of a branch and its upstream.
//!
//! This is the seam. Ahead/behind is the one Tier 0 field whose cost is unbounded and whose
//! primitive is easy to get subtly wrong, so it lives alone: if `gix` reshapes its revision walk,
//! or the counting moves to a different library entirely, this module is what changes. There is
//! deliberately no backend trait — one implementation behind one function is the whole point.
//!
//! # `with_hidden`, not `with_boundary`
//!
//! `with_boundary` reads like `^upstream` and is not. Its own documentation says a boundary "is
//! distinctly different from exclusive revspecs": it stops the walk *at* the given commits but
//! does not hide their ancestors, so any history where upstream has been merged into the local
//! branch is counted twice over. `with_hidden` is the one the docs equate to
//! `^branch-to-not-list`, and it is what this module uses.
//!
//! The trade is cost. `with_hidden` warns that a commit cannot be yielded as soon as it is seen,
//! because it may still turn out to be unwanted, and that disjoint histories may traverse
//! everything. Hence the cap: it is not defensive padding, it is the bound that makes the walk
//! safe to run across a few hundred repositories.

use gix::{ObjectId, revision::walk::Sorting};

use crate::error::{Error, Result};

/// Where the walk stops counting.
///
/// A count equal to this means "at least this many" and renders as `1000+`. A thousand unpushed
/// commits and ten thousand are the same fact to a reader — that something is very wrong — and the
/// difference is not worth an unbounded traversal to establish.
pub const AHEAD_BEHIND_CAP: u32 = 1000;

/// Count commits reachable from `local` but not `upstream`, and the reverse.
///
/// Returns `(ahead, behind)`, each capped at `cap`. Identical tips short-circuit to `(0, 0)`
/// without walking at all, which is the common case across a scanned tree and where most of the
/// Tier 0 budget is saved.
///
/// `cap` is a parameter rather than a constant read directly so the capping behaviour can be
/// tested against a handful of commits instead of a thousand; callers in the scan path pass
/// [`AHEAD_BEHIND_CAP`].
pub fn ahead_behind(
    repo: &gix::Repository,
    local: ObjectId,
    upstream: ObjectId,
    cap: u32,
) -> Result<(u32, u32)> {
    if local == upstream {
        return Ok((0, 0));
    }
    let ahead = count_hidden(repo, local, upstream, cap)?;
    let behind = count_hidden(repo, upstream, local, cap)?;
    Ok((ahead, behind))
}

/// Wrap a `gix` failure as [`Error::Refs`], which stringifies the cause.
fn refs_error(repo: &gix::Repository, err: impl std::fmt::Display) -> Error {
    Error::Refs {
        path: repo.git_dir().to_path_buf(),
        what: "ahead/behind",
        message: err.to_string(),
    }
}

/// Count commits reachable from `tip` but not from `hidden`, stopping at `cap`.
///
/// `BreadthFirst` because it is the cheapest of the three sortings: the other two decode each
/// commit's time to order by it, which this count has no use for. Unlike `with_boundary`,
/// `with_hidden` does not force a sorting of its own, so the choice survives.
fn count_hidden(repo: &gix::Repository, tip: ObjectId, hidden: ObjectId, cap: u32) -> Result<u32> {
    let walk = repo
        .rev_walk([tip])
        .sorting(Sorting::BreadthFirst)
        .with_hidden([hidden])
        .all()
        .map_err(|err| refs_error(repo, err))?;

    let mut count = 0_u32;
    for info in walk {
        // A commit that cannot be decoded fails the count rather than silently shortening it: an
        // ahead/behind that is quietly too low is worse than one reported as unknown.
        info.map_err(|err| refs_error(repo, err))?;
        count += 1;
        if count >= cap {
            break;
        }
    }
    Ok(count)
}
