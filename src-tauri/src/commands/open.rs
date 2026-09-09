//! Launching an external tool for one repository.
//!
//! Three targets, and only one of them is a subprocess-free platform call. Revealing a folder is
//! `tauri-plugin-opener`'s `reveal_item_in_dir`, which is the shell's own API — it initialises COM
//! itself, so it runs happily off the event-loop thread. An editor and a terminal are not: neither
//! has a platform API to ask for, and on Windows the default association for a *folder* is Explorer,
//! so "open with the system default" would open the file manager three times over. They run a
//! command from `settings.json` instead, which makes this the second place in the app that spawns a
//! process, after `git fetch`.
//!
//! # The two Windows traps here are opposites
//!
//! `git fetch` must set `CREATE_NO_WINDOW` or every fetch flashes a console. A **terminal** must
//! not: the console is the entire point of the launch. Same platform, same flag, opposite answers —
//! which is why the flag is decided per target here rather than set once in a helper.
//!
//! The other is program resolution. `Command`'s `PATH` search appends only `.exe`, so
//! `Command::new("code")` cannot find `code.cmd` — the shim every VS Code install actually puts on
//! `PATH` — and fails with "program not found" as though nothing were installed. [`resolve_in`]
//! walks `PATH` against `PATHEXT` instead, which is what the shell does. Going through `cmd.exe /C`
//! would also work and is the obvious fix; it is not used, because it puts a second layer of
//! argument parsing between a repository path and the program meant to receive it.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use anyhow::{Context, anyhow};
use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::{
    error::{CommandError, CommandResult},
    persist::{self, LaunchSpec},
    state::AppState,
};

/// The placeholder a configured command uses for the repository's path.
const PATH_TOKEN: &str = "{path}";

/// Windows' "give this process no console" creation flag.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Where to open a repository.
///
/// Hand-mirrored as a string union in `src/scripts/ipc.ts`, which is where every command signature
/// is written by hand anyway. Three variants do not justify widening `vp run types` to this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpenTarget {
    /// Reveal the folder in Explorer, Finder, or the desktop's file manager.
    FileManager,
    /// Open the folder in the configured editor.
    Editor,
    /// Open a terminal in the folder.
    Terminal,
}

/// Whether the spawned process should be allowed a console window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Console {
    /// An editor. A console here is a flash of black and nothing else.
    Hidden,
    /// A terminal. The console is what the user asked for.
    Shown,
}

/// What a launch will actually run, once the configured command has been substituted.
#[derive(Debug, PartialEq, Eq)]
struct Launch {
    /// The program, as configured.
    program: String,
    /// Its arguments, with [`PATH_TOKEN`] replaced.
    args: Vec<String>,
}

/// Open one repository in an external tool.
///
/// Validates against the **discovered** map rather than the rows — the same superset `refresh_repo`
/// uses, and for a sharper reason. A repository whose HEAD could not be read has no row, and
/// revealing it in the file manager is exactly how a user finds out why it will not open; refusing
/// the one repository they most need to go and look at would be the wrong reading of §6.1.
///
/// The path that reaches the platform is Rust's own copy of the map key, never the string the
/// webview sent. The lookup *is* the validation, so its result is what gets used.
#[tauri::command]
pub async fn open_in(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: PathBuf,
    target: OpenTarget,
) -> CommandResult<()> {
    let found = state
        .discovered(&path)
        .ok_or_else(|| not_a_known_repository(&path))?;
    let path = found.path;

    // Every branch touches the filesystem — a shell lookup, a `PATH` walk, a process creation — and
    // process creation on Windows is slow enough to keep off the event-loop thread even when
    // nothing waits on the result.
    let launched = tauri::async_runtime::spawn_blocking(move || match target {
        OpenTarget::FileManager => tauri_plugin_opener::reveal_item_in_dir(&path)
            .with_context(|| format!("could not reveal `{}`", path.display())),
        OpenTarget::Editor => launch(&persist::open_in(&app).editor, &path, Console::Hidden),
        OpenTarget::Terminal => launch(&persist::open_in(&app).terminal, &path, Console::Shown),
    })
    .await
    .context("the launch did not start")?;

    Ok(launched?)
}

/// Run the configured command against `path`.
fn launch(spec: &LaunchSpec, path: &Path, console: Console) -> anyhow::Result<()> {
    let plan = plan_launch(spec, path)?;
    let program = resolve(&plan.program);

    let mut command = Command::new(&program);
    command.args(&plan.args).current_dir(path);
    set_console(&mut command, console);

    // Spawned and forgotten. Nothing waits on an editor, so there is no status to report and no
    // output to capture; the failure worth reporting is the process not starting, which is what
    // `spawn` itself answers. On Unix the child is left unreaped until the app exits — on Windows,
    // the platform this ships to, there is nothing to reap.
    command
        .spawn()
        .with_context(|| format!("could not run `{}`", plan.program))?;

    tracing::debug!(program = %program.display(), args = ?plan.args, "launched");
    Ok(())
}

/// Turn a configured spec into a concrete program and argument list.
///
/// Pure, and split from the spawn for that reason: substitution and the unconfigured-target refusal
/// are the parts worth testing, and neither of them needs a process.
///
/// `{path}` is replaced wherever it appears and appended as a final argument when it appears
/// nowhere, so `code`, `code {path}` and `wt.exe -d {path}` all do what they look like they do.
fn plan_launch(spec: &LaunchSpec, path: &Path) -> anyhow::Result<Launch> {
    if spec.program.trim().is_empty() {
        return Err(anyhow!(
            "no command is configured for this target — set `openIn` in the app's settings.json"
        ));
    }

    let target = path.display().to_string();
    let mut args: Vec<String> = spec
        .args
        .iter()
        .map(|arg| arg.replace(PATH_TOKEN, &target))
        .collect();
    if !spec.args.iter().any(|arg| arg.contains(PATH_TOKEN)) {
        args.push(target);
    }

    Ok(Launch {
        program: spec.program.clone(),
        args,
    })
}

/// Resolve `program` the way the shell would, falling back to the name itself.
///
/// Falling back rather than failing keeps the error where it belongs: with nothing found,
/// `Command::spawn` produces the platform's own "not found", which is a better message than one
/// invented here.
fn resolve(program: &str) -> PathBuf {
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

/// The searching half of [`resolve`], with the environment passed in so it can be tested.
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
fn resolve_in(program: &str, dirs: &[PathBuf], exts: &[OsString]) -> Option<PathBuf> {
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

/// Apply the console decision. A no-op off Windows, which has no such flag.
#[cfg_attr(
    not(windows),
    expect(unused_variables, reason = "there is no console flag off Windows")
)]
fn set_console(command: &mut Command, console: Console) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;

        if console == Console::Hidden {
            command.creation_flags(CREATE_NO_WINDOW);
        }
    }
}

/// The refusal for a path the app has never discovered.
fn not_a_known_repository(path: &Path) -> CommandError {
    anyhow!("`{}` is not a known repository", path.display()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spec naming the path explicitly gets it exactly where the token was.
    #[test]
    fn the_path_token_is_substituted_where_it_appears() {
        let spec = LaunchSpec {
            program: "wt.exe".into(),
            args: vec!["-d".into(), PATH_TOKEN.into()],
        };

        let plan = plan_launch(&spec, Path::new("C:/work/alpha")).expect("planned");

        assert_eq!(plan.program, "wt.exe");
        assert_eq!(
            plan.args,
            vec!["-d".to_string(), "C:/work/alpha".to_string()]
        );
    }

    /// A spec that never mentions the path still gets it, appended — so `{"program": "code"}` is a
    /// complete configuration rather than a broken one.
    #[test]
    fn a_spec_that_never_names_the_path_gets_it_appended() {
        let spec = LaunchSpec {
            program: "code".into(),
            args: Vec::new(),
        };

        let plan = plan_launch(&spec, Path::new("C:/work/alpha")).expect("planned");

        assert_eq!(plan.args, vec!["C:/work/alpha".to_string()]);
    }

    /// A blank program means "not configured", which is a real state off Windows, where no terminal
    /// default ships. The refusal has to name the file to edit, because nothing in the UI edits it.
    #[test]
    fn an_unconfigured_target_is_refused_with_somewhere_to_go() {
        let spec = LaunchSpec {
            program: "   ".into(),
            args: Vec::new(),
        };

        let error = plan_launch(&spec, Path::new("C:/work/alpha")).expect_err("refused");

        assert!(
            format!("{error:#}").contains("settings.json"),
            "the refusal has to say where to fix it, got: {error:#}"
        );
    }

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
            resolve("definitely-not-installed-anywhere"),
            PathBuf::from("definitely-not-installed-anywhere")
        );
    }

    /// A configured absolute path is not a `PATH` lookup and must not be turned into one.
    #[test]
    fn a_program_given_as_a_path_is_used_as_given() {
        assert_eq!(
            resolve("C:/Program Files/Editor/editor.exe"),
            PathBuf::from("C:/Program Files/Editor/editor.exe")
        );
    }
}
