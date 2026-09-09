//! The IPC surface. Every command is registered in `lib.rs`'s `invoke_handler`.
//!
//! Commands registered this way are callable by every window and need no capability declaration,
//! so `capabilities/default.json` stays at `core:default`. The ACL is enforced only on
//! webview-initiated plugin calls, and the plugins here are reached from Rust — which is exactly
//! why these commands validate their own inputs. A path arriving from the webview is untrusted
//! regardless of where the webview says it came from: `scan_roots` accepts only a configured root,
//! and commands taking a repository path accept only a key of the canonical map.

mod roots;
mod scan;
mod session;

// Glob re-exports, not named ones. `#[tauri::command]` generates a `__cmd__<name>` macro beside
// each function and `generate_handler!` needs both; a named `pub use` carries the function and
// leaves the macro behind, which fails as "cannot find `__cmd__<name>`" at the registration site
// rather than at the export.
pub use roots::*;
pub use scan::*;
pub use session::*;
