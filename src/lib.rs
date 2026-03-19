//! Library entry for the Libra CLI exposing modules and sync/async exec helpers for embedding.

pub mod cli;
pub mod command;
pub mod common_utils;
pub mod git_protocol;
pub mod internal;
pub mod lfs_structs;
pub mod utils;

pub use utils::error::{CliError, CliErrorKind, CliResult};

/// Execute the Libra command in `sync` way.
/// ### Caution
/// There is a tokio runtime inside. Ensure you are NOT in a tokio runtime which can't be nested.
/// ### Example
/// - `["init"]`
/// - `["add", "."]`
pub fn exec(mut args: Vec<&str>) -> CliResult<()> {
    args.insert(0, env!("CARGO_PKG_NAME"));
    cli::parse(Some(&args))
}

/// Execute the Libra command in `async` way.
/// - `async` version of the [exec] function
pub async fn exec_async(mut args: Vec<&str>) -> CliResult<()> {
    args.insert(0, env!("CARGO_PKG_NAME"));
    cli::parse_async(Some(&args)).await
}

#[cfg(test)]
mod tests {
    use serial_test::serial;
    use tempfile::TempDir;

    use crate::utils::test;

    #[test]
    #[serial]
    fn test_libra_init() {
        let tmp_dir = TempDir::new().unwrap();
        let _guard = test::ChangeDirGuard::new(tmp_dir.path());
    }
}
