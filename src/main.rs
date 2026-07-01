//! Binary entry point for the `libra` CLI.
//!
//! Responsibilities, in order:
//! 1. Initialise the tracing subscriber (controlled by `LIBRA_LOG` / `RUST_LOG` and the
//!    optional `LIBRA_LOG_FILE` env var).
//! 2. Spawn a dedicated thread with a 32 MiB stack so deep call chains in the smart
//!    protocol code path do not overflow the much smaller default thread stack.
//! 3. Block on the CLI dispatcher and translate its result into a process exit code,
//!    rendering errors through the same [`OutputConfig`] machinery the dispatcher uses
//!    so that `--json` and friends keep behaving consistently when parsing itself fails.

use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Mutex,
};

use libra::{
    cli,
    utils::{
        error::INTERNAL_ERROR_REPORT_HINT,
        log_config::{LogRotation, resolve_log_config},
        output::OutputConfig,
    },
};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
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
fn init_tracing() {
    let config = resolve_log_config();
    let Some(filter_directive) = config.filter.as_deref() else {
        return;
    };

    let env_filter = build_env_filter(filter_directive);
    let Some(path) = config.file.as_deref() else {
        if let Err(err) = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .try_init()
        {
            eprintln!("warning: failed to initialize tracing subscriber: {err}");
        }
        return;
    };

    match config.rotation {
        // Default / pre-0.7 behaviour: one append-mode file at exactly `path`.
        LogRotation::Never => init_appended_log_file(path, env_filter),
        // lore.md §0.7: roll the file on the requested interval so no single log
        // file grows without limit. Rotation only SPLITS logs by time; it does
        // not delete old files, so total disk use needs external retention
        // (e.g. logrotate) or a dedicated log directory.
        rotation => init_rolling_log_file(path, rotation, env_filter),
    }
}

/// Route tracing to a single append-mode file at `path` (rotation = never).
fn init_appended_log_file(path: &Path, env_filter: EnvFilter) {
    match OpenOptions::new().create(true).append(true).open(path) {
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

/// Route tracing to a time-rolled file: `<dir>/<name>.<date-suffix>` where the
/// suffix granularity follows `rotation`. Blocking writer (no worker guard), so
/// no log lines are lost when the short-lived CLI process exits.
fn init_rolling_log_file(path: &Path, rotation: LogRotation, env_filter: EnvFilter) {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        // A bare filename rolls in the current directory.
        _ => PathBuf::from("."),
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        eprintln!(
            "warning: LIBRA_LOG_FILE {} has no valid UTF-8 file name; tracing disabled",
            path.display()
        );
        return;
    };

    // Create the log directory up front; the builder errors (rather than
    // creating it) if it is missing.
    if let Err(err) = std::fs::create_dir_all(&directory) {
        eprintln!(
            "warning: failed to create log directory {}; tracing disabled: {err}",
            directory.display()
        );
        return;
    }

    let rotation_kind = match rotation {
        LogRotation::Minutely => Rotation::MINUTELY,
        LogRotation::Hourly => Rotation::HOURLY,
        LogRotation::Daily => Rotation::DAILY,
        LogRotation::Never => Rotation::NEVER,
    };

    // Use the fallible builder (not `RollingFileAppender::new`, which panics) so
    // an init failure only disables logging, never crashes the CLI. We do NOT
    // enable `max_log_files` pruning: it deletes by filename prefix and would
    // risk removing unrelated `<file>.*` files in the log directory.
    match RollingFileAppender::builder()
        .rotation(rotation_kind)
        .filename_prefix(file_name)
        .build(&directory)
    {
        Ok(appender) => {
            if let Err(err) = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_ansi(false)
                .with_writer(appender)
                .try_init()
            {
                eprintln!(
                    "warning: failed to initialize rolling tracing subscriber for LIBRA_LOG_FILE {}: {err}",
                    path.display()
                );
            }
        }
        Err(err) => {
            eprintln!(
                "warning: failed to open rolling LIBRA_LOG_FILE {}; tracing disabled: {err}",
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
