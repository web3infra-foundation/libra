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
//!
//! Libra CLI 的库入口。
//!
//! 这个 crate 有两个面：
//! 1. `libra` 二进制文件（见 `main.rs`）解析进程 argv 并分发到 [`cli::parse`]。
//! 2. 嵌入者（集成测试、TUI 和以编程方式驱动 Libra 的外部 Rust crate）调用 [`exec`] 或
//!    [`exec_async`] 并传入预构建的 argv。
//!
//! 下面所有的公共重新导出都是嵌入 API 的一部分，应在补丁发布中保持源代码兼容性。

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
///
/// 同步执行一条 Libra 命令。
///
/// 功能范围：
/// - 将二进制名称（`libra`）附加到 `args`，以便调用者可以使用与 `std::process::Command` 相同的
///   "不带 argv[0] 的 args" 约定。
/// - 启动一个私有多线程 Tokio 运行时，在异步分发器上阻塞，并返回分发器的 `CliResult` 不变。
///
/// 边界条件：
/// - **注意：** 此函数创建自己的 Tokio 运行时。从现有 Tokio 运行时内调用它会崩溃，因为 Tokio
///   运行时无法嵌套。从异步代码，改为调用 [`exec_async`]。
/// - 调用者的 `Vec<&str>` 被消费（由 `insert` 改变）；如果必须保留原始内容，请传入克隆。
///
/// 示例：
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
///
/// [`exec`] 的异步对应物。
///
/// 功能范围：
/// - 使用调用者现有的 Tokio 运行时 — 从任何异步上下文安全地 await。
/// - 将二进制名称附加到 `args`，然后转发到 [`cli::parse_async`]。
///
/// 边界条件：
/// - 来自任何子命令的错误通过 `CliResult::Err` 冒泡；该函数本身不打印错误，将错误渲染留给
///   调用者（通常是 `main.rs`）。
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
    ///
    /// 冒烟测试：验证 [`ChangeDirGuard`](test::ChangeDirGuard) 测试辅助程序可以针对新创建的
    /// 临时目录获取。
    ///
    /// 场景：此守卫是每个改变进程 CWD 的测试的基础。如果守卫无法构造，套件中的所有其他测试都
    /// 不安全运行，所以我们在此处作为金丝雀执行快乐路径。
    #[test]
    #[serial]
    fn test_libra_init() {
        let tmp_dir = TempDir::new().unwrap();
        let _guard = test::ChangeDirGuard::new(tmp_dir.path());
    }
}
