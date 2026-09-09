//! The error type every command returns.
//!
//! `thiserror` gives the engine typed errors; `anyhow` is the glue layer's, and this newtype is
//! what carries an `anyhow::Error` across the IPC boundary. `#[tauri::command]` requires
//! `E: Into<InvokeError>`, which is blanket-implemented for `E: Serialize`, and `anyhow::Error` is
//! not `Serialize`.

/// A command failure, rendered for the webview.
///
/// Serializes as the flattened context chain (`{:#}`) — one string rather than a shape the
/// frontend would have to narrow on. Nothing a command can fail at calls for more: the UI displays
/// the message and there is no branch to take on it.
///
/// This string is the one part of the wire that is not generated from Rust. That is acceptable
/// precisely because `string` cannot drift; generating it would mean either a `ts-rs` derive in
/// `src-tauri` — breaking the `-p repo-scan` scoping that keeps type generation off this crate's
/// dependency tree — or modelling Tauri-edge failures inside the engine, which breaks the boundary
/// the engine exists to enforce.
#[derive(Debug)]
pub struct CommandError(anyhow::Error);

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.0)
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl serde::Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Lets `?` lift an `anyhow::Error` into a command's return type.
///
/// Concrete rather than `impl<E: Into<anyhow::Error>>`: the blanket form collides with the standard
/// library's reflexive `impl From<T> for T`. Commands reach `anyhow` through `.context(..)` or
/// `anyhow!(..)` anyway, so nothing is lost by requiring it explicitly — and requiring it means a
/// bare `io::Error` cannot cross the boundary without someone saying what it was doing.
impl From<anyhow::Error> for CommandError {
    fn from(error: anyhow::Error) -> Self {
        Self(error)
    }
}

/// What every fallible command returns.
pub type CommandResult<T> = std::result::Result<T, CommandError>;
