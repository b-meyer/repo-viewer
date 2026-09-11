//! Telling a user their machine has no WebView2 runtime, instead of showing them nothing.
//!
//! Tauri draws every pixel of this app into a WebView2 control. Without the runtime the window
//! never appears and the process exits, which from the outside is indistinguishable from a crash —
//! and `main.rs` sets `windows_subsystem = "windows"` in release, so there is not even a console
//! for a message to go to. [`ensure`] reads the runtime's version out of the registry before Tauri
//! is given the chance to fail, and puts the reason in a native message box.
//!
//! It is a **diagnostic, not a fix**. The installer's `downloadBootstrapper` is what actually
//! closes the gap for a user who installed the app properly; this is what explains the failure to
//! someone running a build directly, or on a locked-down image where the bootstrapper was skipped.
//!
//! # Why it cannot live in `setup`
//!
//! Windows declared in `tauri.conf.json` are created during `Builder::build()`, which runs before
//! the `setup` hook. By the time `setup` could look, the failure has already happened. So this is
//! called from `run()` ahead of the builder — the one piece of startup work that has to precede it.
//!
//! # Why it is not in the engine
//!
//! `crates/repo-scan/` has no Tauri dependency and no opinion about how anything is drawn. A
//! webview is the application's problem, so the check belongs on this side of the boundary.

/// The Edge updater's registration for the WebView2 Runtime, below the hive-specific prefix.
///
/// The GUID is Microsoft's, fixed, and documented as the way to detect the runtime.
#[cfg(any(windows, test))]
const CLIENT_KEY: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// Refuse to continue when there is no WebView2 runtime to draw into.
///
/// Shows a message box naming the fix and exits, rather than returning — there is nothing a caller
/// could usefully do with the answer, and every path from here ends in a window that cannot be
/// created. A no-op off Windows, where the webview is part of the OS.
pub fn ensure() {
    #[cfg(windows)]
    {
        let (hklm, hkcu) = read_pv();
        if installed(hklm.as_deref(), hkcu.as_deref()) {
            return;
        }

        tracing::error!("no WebView2 runtime registered; cannot create a window");
        report_missing();
        std::process::exit(1);
    }
}

/// Decide whether either registration describes a runtime that is actually present.
///
/// The deciding half of [`ensure`], with the registry values passed in so it can be tested — and
/// tested off Windows, where there is no registry to stage.
///
/// **A present key is not a present runtime.** Microsoft's detection contract is that the value
/// being absent, null, empty **or `0.0.0.0`** all mean not installed; at least one of the two hives
/// must carry a version greater than `0.0.0.0`. A check that only asks whether the read succeeded
/// passes on a machine with no runtime, which is the whole failure this module exists to catch.
///
/// Anything that is neither empty nor all-zero counts as installed, **including a value that does
/// not parse as a version at all.** The bias is deliberate and one-directional: calling a present
/// runtime missing stops an app that would have worked, while calling a missing one present just
/// lets Tauri fail the way it would have anyway.
#[cfg(any(windows, test))]
fn installed(hklm: Option<&str>, hkcu: Option<&str>) -> bool {
    [hklm, hkcu].into_iter().flatten().any(is_real_version)
}

/// Whether one `pv` value describes a runtime rather than a placeholder.
#[cfg(any(windows, test))]
fn is_real_version(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    // All-zero in every component is the documented "registered but not installed" placeholder. A
    // component that does not parse is not zero, so it falls through to `true` by the rule above.
    !trimmed
        .split('.')
        .all(|part| part.parse::<u32>().is_ok_and(|number| number == 0))
}

/// Read the `pv` value from both hives.
///
/// **The HKLM path has to name `WOW6432Node` explicitly.** The Edge updater registers as a 32-bit
/// application, so a 64-bit process reading `HKLM\SOFTWARE\Microsoft\EdgeUpdate\...` looks in the
/// 64-bit view and finds nothing on a machine that has the runtime installed — the check would then
/// refuse to start on every machine it was meant to protect. The non-redirected path is tried
/// afterwards so a 32-bit build still works.
///
/// HKCU is not redirected and needs only the one spelling. Per-user installs land there, and they
/// are the common case for a runtime installed by an app's own bootstrapper rather than by IT.
#[cfg(windows)]
fn read_pv() -> (Option<String>, Option<String>) {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    /// Read one `REG_SZ` `pv` value, treating any failure as absence.
    fn pv(root: winreg::HKEY, path: &str) -> Option<String> {
        RegKey::predef(root)
            .open_subkey(path)
            .and_then(|key| key.get_value::<String, _>("pv"))
            .ok()
    }

    let machine = pv(
        HKEY_LOCAL_MACHINE,
        &format!(r"SOFTWARE\WOW6432Node\{CLIENT_KEY}"),
    )
    .or_else(|| pv(HKEY_LOCAL_MACHINE, &format!(r"SOFTWARE\{CLIENT_KEY}")));

    let user = pv(HKEY_CURRENT_USER, &format!(r"Software\{CLIENT_KEY}"));

    (machine, user)
}

/// Put the reason on screen, in the only channel a windowed process has.
#[cfg(windows)]
fn report_missing() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    /// A NUL-terminated UTF-16 buffer, which is what the `W` entry points take.
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let caption = wide("Repo Viewer cannot start");
    let text = wide(concat!(
        "The Microsoft Edge WebView2 Runtime is not installed, and Repo Viewer needs it to draw \
         its window.\n\n",
        "Install the Evergreen WebView2 Runtime from:\n",
        "https://developer.microsoft.com/microsoft-edge/webview2/\n\n",
        "The Repo Viewer installer normally installs this for you. If you are running the \
         executable straight out of a build directory, install the app instead."
    ));

    // SAFETY: both buffers are NUL-terminated and outlive the call, and a null owner window is
    // documented as "no owner" rather than as an invalid handle.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ordinary case: a per-machine install, nothing under the user's hive.
    #[test]
    fn a_version_under_either_hive_counts_as_installed() {
        assert!(installed(Some("152.0.3179.98"), None));
        assert!(installed(None, Some("152.0.3179.98")));
        assert!(installed(Some("152.0.3179.98"), Some("152.0.3179.98")));
    }

    /// Neither key present is the plain missing-runtime case.
    #[test]
    fn neither_hive_present_is_not_installed() {
        assert!(!installed(None, None));
    }

    /// The trap. The key exists and the read succeeds, so anything that checks only for a value
    /// concludes the runtime is there.
    #[test]
    fn an_all_zero_version_is_not_installed() {
        assert!(!installed(Some("0.0.0.0"), None));
        assert!(!installed(None, Some("0.0.0.0")));
        assert!(!installed(Some("0.0.0.0"), Some("0.0.0.0")));
        assert!(!installed(Some("0"), None));
    }

    /// An empty or whitespace value is documented as meaning the same thing as an absent one.
    #[test]
    fn an_empty_version_is_not_installed() {
        assert!(!installed(Some(""), None));
        assert!(!installed(Some("   "), None));
    }

    /// One real version beside one placeholder still means the runtime is there — a per-machine
    /// install alongside a stale per-user registration is a real shape, and it works.
    #[test]
    fn a_placeholder_beside_a_real_version_is_installed() {
        assert!(installed(Some("0.0.0.0"), Some("152.0.3179.98")));
        assert!(installed(Some("152.0.3179.98"), Some("0.0.0.0")));
    }

    /// A value nothing can parse is treated as present, because the cost of the two mistakes is not
    /// symmetric: refusing to start an app that would have worked is worse than letting Tauri fail.
    #[test]
    fn an_unparseable_version_is_treated_as_installed() {
        assert!(installed(Some("not-a-version"), None));
    }
}
