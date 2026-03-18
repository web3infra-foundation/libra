//! Binary entry point that boots the async runtime, parses CLI arguments, and dispatches execution.

use libra::{cli, utils::output::OutputConfig};
use tracing_subscriber::EnvFilter;

fn main() {
    if std::env::var_os("LIBRA_LOG").is_some() || std::env::var_os("RUST_LOG").is_some() {
        if std::env::var_os("RUST_LOG").is_none()
            && let Some(value) = std::env::var_os("LIBRA_LOG")
        {
            // SAFETY: CLI startup happens before any threads are spawned.
            unsafe {
                std::env::set_var("RUST_LOG", value);
            }
        }

        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .try_init();
    }

    if let Err(err) = cli::parse(None) {
        // Best-effort JSON rendering: resolve the output flags directly from
        // argv so parse-time failures follow the same precedence rules as
        // successful dispatch.
        let argv: Vec<String> = std::env::args().collect();
        let output = OutputConfig::resolve_from_argv(&argv);
        err.print_for_output(&output);
        std::process::exit(err.exit_code());
    }
}
