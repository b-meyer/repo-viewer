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
//! - `tier2` — full file counts. Lazy: expanded rows and explicit refresh only.
//!
//! Tier 2 is not implemented yet — see PLAN.md §11, Phase 4.

pub mod ahead_behind;
pub mod tier0;
pub mod tier1;

pub use ahead_behind::{AHEAD_BEHIND_CAP, ahead_behind};
pub use tier0::{read_tier0, read_tier0_all, read_tier0_all_with};
pub use tier1::{Tier1, Tier1Summary, read_tier1, read_tier1_all_with};
