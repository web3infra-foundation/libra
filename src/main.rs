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

use std::{fs::OpenOptions, path::PathBuf, sync::Mutex};

use libra::{cli, utils::output::OutputConfig};
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
///   stderr (no JSON, since we never got far enough to know the user's preference).
/// - If the CLI thread panics, also exits `1` with a fixed message; thread panics
///   bypass the `CliError` rendering path.
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
                eprintln!("fatal: CLI thread panicked");
                std::process::exit(1);
            }
        },
        Err(err) => {
            eprintln!("fatal: failed to spawn CLI thread: {err}");
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
