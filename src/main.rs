//! Binary entry point that boots the async runtime, parses CLI arguments, and dispatches execution.

use std::{fs::OpenOptions, path::PathBuf, sync::Mutex};

use libra::{cli, utils::output::OutputConfig};
use tracing_subscriber::EnvFilter;

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
        // Best-effort JSON rendering: resolve the output flags directly from
        // argv so parse-time failures follow the same precedence rules as
        // successful dispatch.
        let argv: Vec<String> = std::env::args().collect();
        let output = OutputConfig::resolve_from_argv(&argv);
        err.print_for_output(&output);
        std::process::exit(err.exit_code());
    }
}

fn init_tracing() {
    let log_file = std::env::var_os("LIBRA_LOG_FILE");
    let log_filter = std::env::var_os("LIBRA_LOG")
        .or_else(|| std::env::var_os("RUST_LOG"))
        .or_else(|| log_file.as_ref().map(|_| "libra=debug".into()));
    let Some(log_filter) = log_filter else {
        return;
    };

    let env_filter = EnvFilter::new(log_filter.to_string_lossy());
    let Some(path) = log_file else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .try_init();
        return;
    };

    let path = PathBuf::from(path);
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .try_init();
        }
        Err(err) => {
            eprintln!(
                "warning: failed to open LIBRA_LOG_FILE {}: {err}",
                path.display()
            );
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .try_init();
        }
    }
}
