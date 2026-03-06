//! Binary entry point that boots the async runtime, parses CLI arguments, and dispatches execution.

use libra::cli;

fn main() {
    if let Some(level) = std::env::var_os("LIBRA_LOG").or_else(|| std::env::var_os("RUST_LOG")) {
        let max_level = match level.to_string_lossy().to_ascii_lowercase().as_str() {
            "trace" => tracing::Level::TRACE,
            "debug" => tracing::Level::DEBUG,
            "info" => tracing::Level::INFO,
            "warn" | "warning" => tracing::Level::WARN,
            "error" => tracing::Level::ERROR,
            _ => tracing::Level::INFO,
        };
        let _ = tracing_subscriber::fmt()
            .with_max_level(max_level)
            .try_init();
    }

    if let Err(err) = cli::parse(None) {
        eprintln!("{}", err.render());
        std::process::exit(err.exit_code());
    }
}
