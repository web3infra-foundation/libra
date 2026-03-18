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
        // Best-effort JSON rendering: peek at argv for --json or --machine.
        let argv: Vec<String> = std::env::args().collect();
        let json_mode = argv
            .iter()
            .any(|a| a == "--json" || a.starts_with("--json=") || a == "--machine" || a == "-J");
        if json_mode {
            let pretty = argv
                .iter()
                .any(|a| a == "--json" || a == "--json=pretty" || a == "-J");
            let json = err.render_json();
            if pretty {
                // Re-parse and pretty-print.
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                    if let Ok(formatted) = serde_json::to_string_pretty(&value) {
                        println!("{formatted}");
                    } else {
                        println!("{json}");
                    }
                } else {
                    println!("{json}");
                }
            } else {
                println!("{json}");
            }
        } else {
            err.print_stderr();
        }
        std::process::exit(err.exit_code());
    }
}
