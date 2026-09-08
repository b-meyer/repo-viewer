//! The prune predicate.
//!
//! Two things are cut from the walk, for different reasons. Generated directories are pruned by
//! name because descending them costs far more than everything else combined, and `.git` is pruned
//! because its contents are object storage rather than a tree to search.
//!
//! Only the name-based prunes are counted. `.git` is not an omission — nothing is hidden by
//! skipping it — whereas a directory dropped by name may well have held a repository, which is
//! what [`crate::model::ScanSummary::dirs_pruned`] exists to make visible.

use std::{
    collections::HashSet,
    ffi::OsString,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

use ignore::DirEntry;

/// Build the `filter_entry` predicate.
///
/// Returning `false` from it drops the entry *and* stops the descent, so this cannot be where a
/// repository is recorded — the entry would never reach the visitor. Repository detection and its
/// `WalkState::Skip` live in [`super::discover_roots_with`].
pub(super) fn build(
    prune_names: &[String],
    pruned: Arc<AtomicU32>,
) -> impl Fn(&DirEntry) -> bool + Send + Sync + 'static {
    let names: HashSet<OsString> = prune_names.iter().map(OsString::from).collect();

    move |entry| {
        // Depth 0 is a root the user chose explicitly. Honour it even if it is named `.git` or
        // `target`, rather than silently scanning nothing.
        if entry.depth() == 0 {
            return true;
        }
        // Only directories are worth testing; files are never descended into anyway.
        if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
            return true;
        }

        let name = entry.file_name();
        if name == ".git" {
            return false;
        }
        if names.contains(name) {
            pruned.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }
}
