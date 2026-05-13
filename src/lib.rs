//! Library entry for the Libra CLI.
//!
//! This crate has two faces:
//! 1. The `libra` binary (see `main.rs`) parses the process argv and dispatches to
//!    [`cli::parse`].
//! 2. Embedders (integration tests, the TUI, and external Rust crates that drive
//!    Libra programmatically) call [`exec`] or [`exec_async`] with a pre-built argv.
//!
//! All public re-exports below are part of the embedding API and should remain
//! source-compatible across patch releases.

pub mod cli;
pub mod command;
pub mod common_utils;
pub mod git_protocol;
pub mod internal;
pub mod lfs_structs;
pub mod utils;

pub use utils::error::{CliError, CliErrorKind, CliResult};

/// Execute a Libra command synchronously.
///
/// Functional scope:
/// - Prepends the binary name (`libra`) to `args` so callers can use the same
///   "args without argv\[0\]" convention as `std::process::Command`.
/// - Spins up a private multi-thread Tokio runtime, blocks on the async dispatcher,
///   and returns the dispatcher's `CliResult` unchanged.
///
/// Boundary conditions:
/// - **Caution:** This function creates its own Tokio runtime. Calling it from within
///   an existing Tokio runtime panics because Tokio runtimes cannot be nested. From
///   async code, call [`exec_async`] instead.
/// - The caller's `Vec<&str>` is consumed (mutated by the `insert`); pass a clone if
///   the original must be preserved.
///
/// Examples:
/// - `["init"]`
/// - `["add", "."]`
pub fn exec(mut args: Vec<&str>) -> CliResult<()> {
    args.insert(0, env!("CARGO_PKG_NAME"));
    cli::parse(Some(&args))
}

/// Async counterpart of [`exec`].
///
/// Functional scope:
/// - Uses the caller's existing Tokio runtime — safe to await from any async context.
/// - Prepends the binary name to `args`, then forwards to [`cli::parse_async`].
///
/// Boundary conditions:
/// - Errors from any subcommand bubble up via `CliResult::Err`; the function does not
///   print them itself, leaving error rendering to the caller (typically `main.rs`).
pub async fn exec_async(mut args: Vec<&str>) -> CliResult<()> {
    args.insert(0, env!("CARGO_PKG_NAME"));
    cli::parse_async(Some(&args)).await
}

#[cfg(test)]
mod tests {
    use serial_test::serial;
    use tempfile::TempDir;

    use crate::utils::test;

    /// Smoke test: verifies that the [`ChangeDirGuard`](test::ChangeDirGuard) test
    /// helper can be acquired against a freshly-created temporary directory.
    ///
    /// Scenario: this guard is the foundation of every test that mutates the process
    /// CWD. If the guard cannot construct, every other test in the suite is unsafe to
    /// run, so we exercise the happy path here as a canary.
    #[test]
    #[serial]
    fn test_libra_init() {
        let tmp_dir = TempDir::new().unwrap();
        let _guard = test::ChangeDirGuard::new(tmp_dir.path());
    }
}
