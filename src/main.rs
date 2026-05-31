#![allow(
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    reason = "Bilingual mirrored rustdoc keeps source comments readable but does not always map cleanly to Markdown list indentation"
)]

//! Binary entry point for the `libra` CLI.
//!
//! `libra` CLI 的二进制入口点。
//!
//! Responsibilities, in order:
//! 1. Initialise the tracing subscriber (controlled by `LIBRA_LOG` / `RUST_LOG` and the
//!    optional `LIBRA_LOG_FILE` env var).
//! 2. Spawn a dedicated thread with a 32 MiB stack so deep call chains in the smart
//!    protocol code path do not overflow the much smaller default thread stack.
//! 3. Block on the CLI dispatcher and translate its result into a process exit code,
//!    rendering errors through the same [`OutputConfig`] machinery the dispatcher uses
//!    so that `--json` and friends keep behaving consistently when parsing itself fails.
//!
//! `libra` CLI 的二进制入口点。
//!
//! 职责，按顺序：
//! 1. 初始化追踪订阅者（由 `LIBRA_LOG` / `RUST_LOG` 和可选的 `LIBRA_LOG_FILE` 环境变量控制）。
//! 2. 生成一个专用线程，堆栈大小为 32 MiB，使智能协议代码路径中的深层调用链不会溢出默认线程堆栈。
//! 3. 在 CLI 分发器上阻塞并将其结果翻译为进程退出代码，通过与分发器相同的 [`OutputConfig`] 机制渲染错误，
//!    以便 `--json` 等在解析本身失败时保持一致的行为。

use std::{fs::OpenOptions, path::PathBuf, sync::Mutex};

use libra::{
    cli,
    utils::{error::INTERNAL_ERROR_REPORT_HINT, output::OutputConfig},
};
use tracing_subscriber::EnvFilter;

/// Process entry point.
///
/// Functional scope:
/// - Sets up logging, runs the CLI on a high-stack thread, and translates any error
///   into a non-zero exit code. The function intentionally does not return a
///   `Result` — exit codes are the only meaningful surface for a binary entry point.
///
/// Boundary conditions:
/// - If the CLI thread fails to spawn, exits with code `1` and a fatal message on
///   stderr (no JSON, since we never got far enough to know the user's preference)
///   plus the standard internal-error report hint.
/// - If the CLI thread panics, also exits `1` with a fixed message plus the same
///   hint; thread panics bypass the `CliError` rendering path.
/// - On a clean `Err(CliError)`, the exit code is sourced from
///   [`CliError::exit_code`] so each error class has a stable code.
///
/// 进程入口点。
///
/// 功能范围：
/// - 设置日志记录，在高堆栈线程上运行 CLI，并将任何错误转换为非零退出代码。该函数有意不返回
///   `Result` — 退出代码是二进制入口点的唯一有意义的表面。
///
/// 边界条件：
/// - 如果 CLI 线程生成失败，则以代码 `1` 退出并在 stderr 上显示致命消息（无 JSON，因为我们
///   从未达到足够了解用户偏好的程度）以及标准的内部错误报告提示。
/// - 如果 CLI 线程崩溃，也以代码 `1` 退出，并显示固定消息加上相同的提示；线程崩溃绕过 `CliError`
///   渲染路径。
/// - 在干净的 `Err(CliError)` 上，退出代码源自 [`CliError::exit_code`]，因此每个错误类都有一个稳定的代码。
fn main() {
    init_tracing();

    const CLI_STACK_SIZE: usize = 32 * 1024 * 1024;
    let handle = std::thread::Builder::new()
        .name("libra-cli".to_string())
        .stack_size(CLI_STACK_SIZE)
        .spawn(|| cli::parse(None));

    let result = match handle {
        Ok(handle) => match handle.join() {
            Ok(result) => result,
            Err(_) => {
                eprintln!("fatal: CLI thread panicked\n\nHint: {INTERNAL_ERROR_REPORT_HINT}");
                std::process::exit(1);
            }
        },
        Err(err) => {
            eprintln!(
                "fatal: failed to spawn CLI thread: {err}\n\nHint: {INTERNAL_ERROR_REPORT_HINT}"
            );
            std::process::exit(1);
        }
    };

    if let Err(err) = result {
        // Best-effort JSON rendering: resolve the output flags directly from argv so
        // parse-time failures follow the same precedence rules as successful dispatch.
        // We must read from `std::env::args()` (not the dispatcher's parsed `args`)
        // because the dispatcher returned an error before producing them.
        let argv: Vec<String> = std::env::args().collect();
        let output = OutputConfig::resolve_from_argv(&argv);
        err.print_for_output(&output);
        std::process::exit(err.exit_code());
    }
}

/// Configure the global [`tracing`] subscriber.
///
/// Functional scope:
/// - Reads the filter directive from `LIBRA_LOG`, falling back to `RUST_LOG`, falling
///   back to `libra=debug` only when `LIBRA_LOG_FILE` is set (so the file is never
///   created with no useful content).
/// - When `LIBRA_LOG_FILE` is set, opens that file in append mode and routes events
///   there with ANSI escapes disabled. Otherwise emits to stderr with default
///   formatting.
///
/// Boundary conditions:
/// - When no env vars are set, returns silently without installing any subscriber so
///   that ordinary CLI use produces no log noise.
/// - Subscriber installation is best-effort: if the global subscriber is already
///   installed (e.g. because a library consumer set one up first) we print a warning
///   to stderr but never fail the process.
/// - If `LIBRA_LOG_FILE` cannot be opened, we warn on stderr and leave tracing
///   disabled — we never crash the CLI just because logging failed.
///
/// 配置全局 [`tracing`] 订阅者。
///
/// 功能范围：
/// - 从 `LIBRA_LOG` 读取过滤器指令，回退到 `RUST_LOG`，仅在设置 `LIBRA_LOG_FILE` 时回退到 `libra=debug`
///   （这样文件永远不会以没有有用内容的方式创建）。
/// - 当设置 `LIBRA_LOG_FILE` 时，以追加模式打开该文件并将事件路由到该文件，禁用 ANSI 转义。
///   否则以默认格式化方式发送到 stderr。
///
/// 边界条件：
/// - 当没有环境变量被设置时，默认情况下返回而不安装任何订阅者，以便普通 CLI 使用不产生日志噪声。
/// - 订阅者安装是尽力而为的：如果全局订阅者已经被安装（例如，因为库消费者首先设置了），我们会在
///   stderr 上打印警告，但永远不会导致进程失败。
/// - 如果 `LIBRA_LOG_FILE` 无法打开，我们会在 stderr 上警告并禁用追踪 — 我们永远不会仅因为日志记录
///   失败就崩溃 CLI。
fn init_tracing() {
    let log_file = std::env::var_os("LIBRA_LOG_FILE");
    let log_filter = std::env::var_os("LIBRA_LOG")
        .or_else(|| std::env::var_os("RUST_LOG"))
        .or_else(|| log_file.as_ref().map(|_| "libra=debug".into()));
    let Some(log_filter) = log_filter else {
        return;
    };

    let env_filter = build_env_filter(&log_filter.to_string_lossy());
    let Some(path) = log_file else {
        if let Err(err) = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .try_init()
        {
            eprintln!("warning: failed to initialize tracing subscriber: {err}");
        }
        return;
    };

    let path = PathBuf::from(path);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            if let Err(err) = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .try_init()
            {
                eprintln!(
                    "warning: failed to initialize tracing subscriber for LIBRA_LOG_FILE {}: {err}",
                    path.display()
                );
            }
        }
        Err(err) => {
            eprintln!(
                "warning: failed to open LIBRA_LOG_FILE {}; tracing disabled: {err}",
                path.display()
            );
        }
    }
}

/// Build the [`EnvFilter`] that drives the global tracing subscriber.
///
/// Functional scope:
/// - Parses `directives` (the resolved value of `LIBRA_LOG`/`RUST_LOG`/the
///   `libra=debug` fallback) and, when the user did not say anything about
///   the `rfuse3` target, pins `rfuse3::raw::session=error` so the spammy
///   `"The data is not 4096 bytes aligned"` warning that fires for every
///   sub-page write to the worktree FUSE mount stays out of normal logs.
///
/// Boundary conditions:
/// - If the user opts in by mentioning `rfuse3` anywhere in their filter
///   string (e.g. `LIBRA_LOG=rfuse3=warn`), we skip the suppression so the
///   user's directive wins outright.
/// - The added directive is a static literal whose parse cannot fail in any
///   supported `tracing-subscriber` version; the `expect` is a hard
///   invariant, not a runtime fallback.
///
/// 构建驱动全局追踪订阅者的 [`EnvFilter`]。
///
/// 功能范围：
/// - 解析 `directives`（`LIBRA_LOG`/`RUST_LOG`/`libra=debug` 回退的已解析值），当用户没有
///   提及 `rfuse3` 目标时，锁定 `rfuse3::raw::session=error` 以使每次对工作树 FUSE 挂载的
///   子页写入触发的嘈杂 `"The data is not 4096 bytes aligned"` 警告不会出现在正常日志中。
///
/// 边界条件：
/// - 如果用户通过在其过滤器字符串中的任何地方提及 `rfuse3` 来选择加入（例如
///   `LIBRA_LOG=rfuse3=warn`），我们跳过抑制，以便用户的指令直接获胜。
/// - 添加的指令是一个静态字面量，其解析在任何受支持的 `tracing-subscriber` 版本中都不会失败；
///   `expect` 是一个硬不变量，而不是运行时回退。
fn build_env_filter(directives: &str) -> EnvFilter {
    let env_filter = EnvFilter::new(directives);
    if directives.contains("rfuse3") {
        return env_filter;
    }
    env_filter.add_directive(
        "rfuse3::raw::session=error"
            .parse()
            .expect("static rfuse3 directive must parse"),
    )
}
