//! Binary entry point that boots the async runtime, parses CLI arguments, and dispatches execution.

use libra::cli;
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
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}
