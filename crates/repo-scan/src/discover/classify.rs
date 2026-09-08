//! Deciding whether a directory is a repository, and which flavour.
//!
//! All four of the §5.2 cases — `.git` as a file, linked worktrees, submodules, and bare repos —
//! are handled by `gix::discover::is_git`, which is `gix_discover` re-exported wholesale and needs
//! no feature flag. It follows a `.git` _file_ to the private directory it names and then requires
//! a valid HEAD, an `objects/` directory and a `refs/` directory, so the "test for existence, then
//! resolve" rule is its contract rather than something reimplemented here.
//!
//! Nothing in this module opens a repository. Classification is filesystem metadata only; the
//! first `gix::open` happens in Tier 0.

use std::path::{Path, PathBuf};

use gix::discover::repository::Kind;

use crate::model::RepoKind;

/// A directory that turned out to be a repository.
#[derive(Debug)]
pub(super) struct Classified {
    /// Which flavour.
    pub kind: RepoKind,
    /// The resolved Git directory, before canonicalisation.
    pub git_dir: PathBuf,
}

/// Probe `dir` for a repository.
///
/// `Ok(None)` means "an ordinary directory, keep walking". `Err` means the directory looked like a
/// repository and could not be read as one — a broken `.git` or a permission failure. Those become
/// [`crate::model::ScanError`] values rather than ending the scan, but they are not silently
/// dropped: a repository the user expects to see would otherwise just be absent.
pub(super) fn classify(dir: &Path) -> Result<Option<Classified>, String> {
    let dot_git = dir.join(".git");

    // `try_exists` rather than `exists`, which folds a permission failure into `false` and would
    // report an unreadable repository as an ordinary directory.
    match dot_git.try_exists() {
        Ok(true) => match gix::discover::is_git(&dot_git) {
            Ok(kind) => Ok(Some(from_kind(kind, dot_git))),
            Err(err) => Err(err.to_string()),
        },
        Ok(false) => classify_bare(dir),
        Err(err) => Err(err.to_string()),
    }
}

/// The bare case: no `.git` at all, so the directory *is* the Git directory.
///
/// Gated on `HEAD` first. That is one extra `stat` per directory walked, and it fails for
/// essentially every directory in a real tree, so the `objects/` and `refs/` checks — and the much
/// more expensive `is_git` — are only ever paid for a genuine candidate.
fn classify_bare(dir: &Path) -> Result<Option<Classified>, String> {
    if !dir.join("HEAD").is_file() {
        return Ok(None);
    }
    if !dir.join("objects").is_dir() || !dir.join("refs").is_dir() {
        // A directory that merely happens to contain a file called `HEAD`.
        return Ok(None);
    }
    match gix::discover::is_git(dir) {
        Ok(kind) => Ok(Some(from_kind(kind, dir.to_path_buf()))),
        Err(err) => Err(err.to_string()),
    }
}

/// Map `gix`'s guess onto [`RepoKind`], carrying the Git directory it resolved.
///
/// `probed` is the path handed to `is_git`: `<dir>/.git` in the normal case, `<dir>` in the bare
/// one. It is the fallback Git directory for the variants that do not name one.
fn from_kind(kind: Kind, probed: PathBuf) -> Classified {
    match kind {
        // A `.git` directory in the main worktree.
        Kind::WorkTree {
            linked_git_dir: None,
        } => Classified {
            kind: RepoKind::Normal,
            git_dir: probed,
        },
        // A `.git` *file* whose private directory has a `commondir` — the object store is shared
        // with the main worktree, which is exactly what makes this a linked worktree and not a
        // second copy of the same repository.
        Kind::WorkTree {
            linked_git_dir: Some(git_dir),
        } => Classified {
            kind: RepoKind::LinkedWorktree,
            git_dir,
        },
        // A `.git` file with no `commondir` beside it.
        Kind::Submodule { git_dir } => Classified {
            kind: RepoKind::Submodule,
            git_dir,
        },
        Kind::PossiblyBare => Classified {
            kind: RepoKind::Bare,
            git_dir: probed,
        },
        // Both of these are the git-dir side of the pair — `.git/worktrees/<name>` and
        // `.git/modules/<name>`. The walk never descends into `.git`, so neither is reachable from
        // a scan; they are mapped rather than ignored so that pointing a root straight at one
        // still produces a row.
        Kind::WorkTreeGitDir { .. } => Classified {
            kind: RepoKind::LinkedWorktree,
            git_dir: probed,
        },
        Kind::SubmoduleGitDir => Classified {
            kind: RepoKind::Submodule,
            git_dir: probed,
        },
    }
}
