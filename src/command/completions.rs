//! `libra completions <shell>` — generate a shell completion script for the
//! Libra CLI, matching the ergonomics of Lore's `completions` command and the
//! `git completion` contrib scripts.
//!
//! The script is produced by `clap_complete` from the live clap command tree,
//! so it always tracks the actual subcommand/flag surface without a hand-kept
//! table. It touches no repository state and works outside a repository.

use std::io::Write;

use clap::Parser;
use clap_complete::Shell;
use serde::Serialize;

use crate::utils::output::{OutputConfig, emit_json_data};

pub const COMPLETIONS_EXAMPLES: &str = "\
EXAMPLES:
    libra completions bash > /etc/bash_completion.d/libra   Install bash completion
    libra completions zsh > ~/.zsh/completions/_libra        Install zsh completion
    libra completions fish > ~/.config/fish/completions/libra.fish
    eval \"$(libra completions bash)\"                         Load into the current shell
    libra --json completions bash                            Structured { shell, script }";

/// Generate a shell completion script for `libra`.
#[derive(Parser, Debug)]
#[command(after_help = COMPLETIONS_EXAMPLES)]
pub struct CompletionsArgs {
    /// Shell to generate a completion script for.
    #[arg(value_enum, value_name = "SHELL")]
    pub shell: Shell,
}

#[derive(Debug, Serialize)]
struct CompletionsOutput {
    shell: String,
    script: String,
}

/// Generate the completion script for `args.shell`.
///
/// # Arguments
/// * `args` - the requested shell.
/// * `cmd` - the fully-built root clap command (`Cli::command()`), passed in
///   so this module stays decoupled from the private CLI struct.
/// * `output` - global output config; `--json`/`--machine` wrap the script in a
///   `{ shell, script }` envelope, otherwise the raw script is written to stdout.
pub fn execute_safe(
    args: CompletionsArgs,
    mut cmd: clap::Command,
    output: &OutputConfig,
) -> crate::utils::error::CliResult<()> {
    // The tool is always invoked as `libra`, independent of the crate name.
    const BIN_NAME: &str = "libra";

    let mut buffer: Vec<u8> = Vec::new();
    clap_complete::generate(args.shell, &mut cmd, BIN_NAME, &mut buffer);
    // Completion scripts are valid UTF-8; lossy conversion is a safe fallback.
    let script = String::from_utf8_lossy(&buffer).into_owned();

    if output.is_json() {
        emit_json_data(
            "completions",
            &CompletionsOutput {
                shell: args.shell.to_string(),
                script,
            },
            output,
        )
    } else {
        // Write the script verbatim so it can be piped directly to a file or
        // `eval`'d; do not add or strip a trailing newline.
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(script.as_bytes()).map_err(|e| {
            crate::utils::error::CliError::io(format!("failed to write completion script: {e}"))
        })?;
        // Propagate flush failures too: a script left in the buffer would be a
        // silently truncated completion file despite a success exit code.
        handle.flush().map_err(|e| {
            crate::utils::error::CliError::io(format!("failed to flush completion script: {e}"))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    // A tiny stand-in root command so the unit test does not depend on the
    // private `Cli` struct in `cli.rs`; the real dispatch passes `Cli::command()`.
    #[derive(Parser)]
    #[command(name = "libra")]
    struct FakeRoot {
        #[arg(long)]
        verbose: bool,
    }

    fn render(shell: Shell) -> String {
        let mut buffer = Vec::new();
        let mut cmd = FakeRoot::command();
        clap_complete::generate(shell, &mut cmd, "libra", &mut buffer);
        String::from_utf8(buffer).expect("completion script is UTF-8")
    }

    #[test]
    fn bash_script_mentions_binary() {
        let script = render(Shell::Bash);
        assert!(script.contains("libra"), "bash script should mention libra");
    }

    #[test]
    fn every_shell_generates_nonempty_script() {
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            assert!(
                !render(shell).is_empty(),
                "{shell} completion script should be non-empty"
            );
        }
    }
}
