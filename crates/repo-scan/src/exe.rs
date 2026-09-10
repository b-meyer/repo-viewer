//! Finding an executable the way the shell would.
//!
//! `Command`'s own `PATH` search appends only `.exe`, so `Command::new("code")` cannot find
//! `code.cmd` — the shim every VS Code install actually puts on `PATH` — and fails with "program
//! not found" as though nothing were installed. [`resolve_in`] walks `PATH` against `PATHEXT`
//! instead, which is what the shell does. Going through `cmd.exe /C` would also work and is the
//! obvious fix; it is not used, because it puts a second layer of argument parsing between a path
//! and the program meant to receive it.
//!
//! Two callers need this and they are in different crates: [`crate::fetch`] resolves `git`, and
//! `src-tauri`'s `commands/open.rs` resolves the configured editor and terminal. It lives here
//! because the engine cannot reach into `src-tauri`, and because `examples/scan.rs` has to find
//! `git` with no application around it.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Resolve `program` the way the shell would, falling back to the name itself.
///
/// Falling back rather than failing keeps the error where it belongs: with nothing found,
/// `Command::spawn` produces the platform's own "not found", which is a better message than one
/// invented here.
pub fn resolve_program(program: &str) -> PathBuf {
    let named = Path::new(program);
    // An absolute or relative path, or a name that already carries its extension, is not a `PATH`
    // lookup at all — `Command` handles both correctly.
    if named.components().count() > 1 || named.extension().is_some() {
        return named.to_path_buf();
    }

    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    let exts: Vec<OsString> = std::env::var_os("PATHEXT")
        .map(|exts| {
            exts.to_string_lossy()
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(OsString::from)
                .collect()
        })
        .unwrap_or_default();

    resolve_in(program, &dirs, &exts).unwrap_or_else(|| named.to_path_buf())
}

/// The searching half of [`resolve_program`], with the environment passed in so it can be tested.
///
/// Extensions are tried in `PATHEXT` order within each directory, which is the shell's own
/// precedence: a `code.exe` beside a `code.cmd` in one folder wins, and a `code.cmd` earlier on
/// `PATH` beats a `code.exe` later.
///
/// **The extensions come before the bare name, and that order is load-bearing.** A real VS Code
/// install puts *both* `code` and `code.cmd` in one directory, where the extensionless one is a
/// shell script for Git Bash — and Windows cannot execute it, so preferring it finds a file and then
/// fails to run it. `cmd.exe` does not consider extensionless files at all. The bare name stays as
/// the last resort, which is what makes this correct off Windows too: there `PATHEXT` is unset,
/// `exts` is empty, and the bare name is reached immediately.
///
/// The path that comes back carries the casing of the extension that matched rather than the one on
/// disk, because Windows compares them case-insensitively — `code.CMD` for a `code.cmd`. That is
/// only ever passed straight to `Command`, which compares the same way.
pub fn resolve_in(program: &str, dirs: &[PathBuf], exts: &[OsString]) -> Option<PathBuf> {
    for dir in dirs {
        for ext in exts {
            let mut name = OsString::from(program);
            name.push(ext);
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        let bare = dir.join(program);
        if bare.is_file() {
            return Some(bare);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trap this function exists for: a `.cmd` shim is what a VS Code install puts on `PATH`,
    /// and `Command`'s own search would never find it.
    #[test]
    fn a_shim_with_a_pathext_extension_is_found() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("code.cmd"), "@echo off").expect("write the shim");

        let found = resolve_in(
            "code",
            &[dir.path().to_path_buf()],
            &[OsString::from(".EXE"), OsString::from(".CMD")],
        )
        .expect("the shim is found");

        assert!(found.is_file());
        assert!(
            found
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("code.cmd")),
            "got {found:?}"
        );
    }

    /// `PATHEXT` order decides between two candidates in one directory, as it does in the shell.
    #[test]
    fn pathext_order_decides_between_two_candidates() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("code.cmd"), "@echo off").expect("write");
        std::fs::write(dir.path().join("code.exe"), "MZ").expect("write");

        let found = resolve_in(
            "code",
            &[dir.path().to_path_buf()],
            &[OsString::from(".EXE"), OsString::from(".CMD")],
        )
        .expect("one of them is found");

        assert!(
            found
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("code.exe")),
            "PATHEXT order decides, got {found:?}"
        );
    }

    /// The shape a real VS Code install actually has: an extensionless `code` shell script beside
    /// `code.cmd`, in one directory. Windows cannot execute the first of those, so finding it is
    /// worse than finding nothing — the launch would fail with a message about a program that is
    /// plainly installed.
    #[test]
    fn an_extension_wins_over_an_extensionless_file_of_the_same_name() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("code"), "#!/bin/sh").expect("the shell script");
        std::fs::write(dir.path().join("code.cmd"), "@echo off").expect("the shim");

        let found = resolve_in(
            "code",
            &[dir.path().to_path_buf()],
            &[OsString::from(".EXE"), OsString::from(".CMD")],
        )
        .expect("one of them is found");

        assert!(
            found
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("code.cmd")),
            "the extensionless script is not executable on Windows, got {found:?}"
        );
    }

    /// And with no extension to be had, the bare name is still the answer — which is the only case
    /// off Windows, where `PATHEXT` does not exist.
    #[test]
    fn the_bare_name_is_the_last_resort_rather_than_the_first() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("editor"), "#!/bin/sh").expect("the executable");

        assert_eq!(
            resolve_in("editor", &[dir.path().to_path_buf()], &[]),
            Some(dir.path().join("editor"))
        );
    }

    /// Nothing found is not an error here: the spawn produces the platform's own message, which is
    /// better than one invented in this file.
    #[test]
    fn an_unresolvable_program_falls_back_to_its_own_name() {
        assert_eq!(
            resolve_program("definitely-not-installed-anywhere"),
            PathBuf::from("definitely-not-installed-anywhere")
        );
    }

    /// A configured absolute path is not a `PATH` lookup and must not be turned into one.
    #[test]
    fn a_program_given_as_a_path_is_used_as_given() {
        assert_eq!(
            resolve_program("C:/Program Files/Editor/editor.exe"),
            PathBuf::from("C:/Program Files/Editor/editor.exe")
        );
    }
}
