//! Tiered Git reads: turning a [`DiscoveredRepo`](crate::model::DiscoveredRepo) into a
//! [`RepoStatus`](crate::model::RepoStatus).
//!
//! Discovery answers "where are the repositories". This module answers "what state is each one
//! in", and it does so in tiers because the costs are wildly uneven: refs are essentially free,
//! the dirty flag needs an ignore-aware worktree walk, and full counts need a complete
//! index-to-worktree diff. Streaming them separately is what lets a row appear before any
//! worktree is touched.
//!
//! - [`tier0`] — refs only. Branch, upstream, ahead/behind, stash, state, tip commit,
//!   last-fetched age. Runs on every scan and must stay refs-only.
//! - [`tier1`] — the dirty flag and conflicted count. Streams right after Tier 0.
//! - [`tier2`] — full file counts and the submodule list. Lazy: expanded rows and explicit
//!   refresh only, one repository at a time. It is the only tier with no fan-out, deliberately —
//!   see its module docs.

pub mod ahead_behind;
pub mod tier0;
pub mod tier1;
pub mod tier2;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A cancellation flag for one `gix` status walk, seeded from the caller's.
///
/// **Never hand a shared flag to `should_interrupt_owned`.** `gix` treats an owned flag as one it
/// may write to: `parallel_iter_drop` does `should_interrupt.swap(true, ..)` to stop its worker
/// threads when the iterator is dropped, and only afterwards tries to restore the previous value.
/// A flag shared across concurrent walks therefore reads `true` inside that window for *every*
/// other walk holding it — so one repository's early exit aborts whichever of its neighbours were
/// mid-walk, and they report `Interrupted` with nothing having asked them to stop.
///
/// It is worse than a spurious per-repository error. `read_tier1_all_with` checks the same flag
/// between repositories, and `src-tauri`'s pipeline checks it after each batch and breaks out of
/// the scan — so a transient `true` can end a scan early and report it as cancelled.
///
/// Seeding from the caller's flag keeps the part that matters: a walk that starts after the user
/// cancels still stops immediately. What is given up is interrupting a walk already in progress,
/// which the fan-out's between-repository check covers at a coarser grain, and which Tier 1's
/// early exit makes short anyway.
pub(crate) fn private_interrupt(shared: &Arc<AtomicBool>) -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(shared.load(Ordering::Relaxed)))
}

pub use ahead_behind::{AHEAD_BEHIND_CAP, ahead_behind};
pub use tier0::{read_tier0, read_tier0_all, read_tier0_all_with};
pub use tier1::{Tier1, Tier1Summary, read_tier1, read_tier1_all_with};
pub use tier2::{Tier2, read_tier2};
