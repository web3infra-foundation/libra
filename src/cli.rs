//! CLI entry for Libra, defining clap subcommands, setting the hash algorithm from config,
//! and dispatching each command handler.

use std::{env, io::Write, path::Path};

use clap::{
    Parser, Subcommand,
    error::{ContextKind, ContextValue, ErrorKind},
};
use git_internal::hash::{HashKind, set_hash_kind};

use crate::{
    command,
    internal::{config::ConfigKv, db},
    utils,
    utils::{
        error::{CliError, CliResult},
        output::OutputConfig,
    },
};

const ROOT_AFTER_HELP: &str = "\
Help Topics:
  error-codes  Print the stable CLI error code table (`libra help error-codes`)

Output Examples:
  libra --json status
  libra --json branch
";

const ERROR_CODES_HELP: &str = include_str!("../docs/error-codes.md");

/// Reads the repository's configuration and sets the global hash kind.
/// This must be called for any command that operates within an existing repository.
/// Returns an error if the repository database is missing or corrupted.
async fn set_local_hash_kind_for_storage(storage: &Path) -> CliResult<()> {
    let db_path = storage.join(utils::util::DATABASE);
    if !db_path.exists() {
        return Err(CliError::fatal(format!(
            "repository database not found at '{}'",
            db_path.display()
        )));
    }

    let db_conn = db::get_db_conn_instance_for_path(&db_path)
        .await
        .map_err(|e| {
            CliError::fatal(format!(
                "failed to open repository database '{}': {}",
                db_path.display(),
                e
            ))
        })?;
    let object_format = ConfigKv::get_with_conn(&db_conn, "core.objectformat")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        .unwrap_or_else(|| "sha1".to_string());

    let hash_kind = match object_format.as_str() {
        "sha1" => HashKind::Sha1,
        "sha256" => HashKind::Sha256,
        _ => {
            return Err(CliError::fatal(format!(
                "unsupported object format: '{object_format}'"
            )));
        }
    };
    set_hash_kind(hash_kind);
    Ok(())
}

// The Cli struct represents the root of the command line interface.
#[derive(Parser, Debug)]
#[command(
    about = "Libra: An AI native version control system for monorepo and trunk-based development.",
    version = env!("CARGO_PKG_VERSION"),
    after_help = ROOT_AFTER_HELP
)]
struct Cli {
    /// Emit machine-readable JSON to stdout.
    /// Use `--json` alone for pretty output, or `--json=compact` / `--json=ndjson`
    /// to select an alternative layout.  The `=` is required when specifying a format
    /// so that the subcommand name is not consumed as the value.
    #[arg(
        long,
        short = 'J',
        global = true,
        value_name = "FORMAT",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "pretty",
        value_parser = ["pretty", "compact", "ndjson"],
    )]
    json: Option<String>,

    /// Strict machine mode.
    /// Implies --json=ndjson --no-pager --color=never --quiet.
    /// Disables all prompts and decorative text.
    #[arg(long, global = true)]
    machine: bool,

    /// Disable automatic pager (less) for long output.
    #[arg(long, global = true)]
    no_pager: bool,

    /// When to use terminal colors.
    /// Also respects the NO_COLOR environment variable (see <https://no-color.org>).
    #[arg(
        long,
        global = true,
        value_name = "WHEN",
        default_value = "auto",
        value_parser = ["auto", "never", "always"],
    )]
    color: String,

    /// Suppress standard stdout output; keep warnings/errors on stderr.
    /// This includes primary command results, unlike some Git per-command
    /// `--quiet` flags that only suppress informational chatter.
    #[arg(long, short = 'q', global = true)]
    quiet: bool,

    /// Return non-zero exit code (exit 9) when a warning is emitted.
    #[arg(long, global = true)]
    exit_code_on_warning: bool,

    /// Control progress output for long-running operations.
    /// `json` emits NDJSON progress events; `text` shows a human-friendly bar;
    /// `none` suppresses progress entirely.
    #[arg(
        long,
        global = true,
        value_name = "MODE",
        default_value = "auto",
        value_parser = ["json", "text", "none", "auto"],
    )]
    progress: String,

    #[command(subcommand)]
    command: Commands,
}

/// The Commands enum represents the subcommands that can be used with the CLI.
/// subcommand's execute and args are defined in `command` module
#[derive(Subcommand, Debug)]
enum Commands {
    // Each variant of the enum represents a subcommand.
    // The about attribute provides a brief description of the subcommand.
    // The arguments of the subcommand are defined in the command module.

    // Init and Clone are the only commands that can be executed without a repository
    #[command(about = "Initialize a new repository")]
    Init(command::init::InitArgs),
    #[command(about = "Clone a repository into a new directory")]
    Clone(command::clone::CloneArgs),
    #[command(about = "Start Libra Code interactive TUI (with background web server)")]
    Code(command::code::CodeArgs),
    // The rest of the commands require a repository to be present
    #[command(about = "Add file contents to the index")]
    Add(command::add::AddArgs),
    #[command(
        about = "Remove files from the working tree and from the index",
        alias = "remove",
        alias = "delete"
    )]
    Rm(command::remove::RemoveArgs),
    #[command(about = "Restore working tree files", alias = "unstage")]
    Restore(command::restore::RestoreArgs),
    #[command(about = "Show the working tree status", alias = "st")]
    Status(command::status::StatusArgs),
    #[command(about = "Remove untracked files from the working tree")]
    Clean(command::clean::CleanArgs),
    #[command(
        subcommand,
        about = "Stash the changes in a dirty working directory away"
    )]
    Stash(Stash),
    #[command(subcommand, about = "Large File Storage")]
    Lfs(command::lfs::LfsCmds),
    #[command(about = "Show commit logs", alias = "hist", alias = "history")]
    Log(command::log::LogArgs),
    #[command(about = "Summarize 'git log' output", alias = "slog")]
    Shortlog(command::shortlog::ShortlogArgs),
    #[command(about = "Show various types of objects")]
    Show(command::show::ShowArgs),
    #[command(about = "List references in a local repository")]
    ShowRef(command::show_ref::ShowRefArgs),
    #[command(about = "List, create, or delete branches", alias = "br")]
    Branch(command::branch::BranchArgs),
    #[command(about = "Create a new tag")]
    Tag(command::tag::TagArgs),
    #[command(about = "Record changes to the repository", alias = "ci")]
    Commit(command::commit::CommitArgs),
    #[command(about = "Switch branches", alias = "sw")]
    Switch(command::switch::SwitchArgs),
    #[command(about = "Reapply commits on top of another base tip", alias = "rb")]
    Rebase(command::rebase::RebaseArgs),
    #[command(about = "Merge changes")]
    Merge(command::merge::MergeArgs),
    #[command(about = "Reset current HEAD to specified state")]
    Reset(command::reset::ResetArgs),
    #[command(about = "Move or rename a file, a directory, or a symlink")]
    Mv(command::mv::MvArgs),
    #[command(
        about = "Give an object a human readable name based on an available ref",
        alias = "desc"
    )]
    Describe(command::describe::DescribeArgs),
    #[command(
        about = "Apply the changes introduced by some existing commits",
        alias = "cp"
    )]
    CherryPick(command::cherry_pick::CherryPickArgs),
    #[command(about = "Update remote refs along with associated objects")]
    Push(command::push::PushArgs),
    #[command(about = "Download objects and refs from another repository")]
    Fetch(command::fetch::FetchArgs),
    #[command(about = "Fetch from and integrate with another repository or a local branch")]
    Pull(command::pull::PullArgs),
    #[command(about = "Show changes between commits, commit and working tree, etc")]
    Diff(command::diff::DiffArgs),
    #[command(about = "Search for patterns in tracked files")]
    Grep(command::grep::GrepArgs),
    #[command(about = "Show author and history of each line of a file")]
    Blame(command::blame::BlameArgs),
    #[command(about = "Revert some existing commits")]
    Revert(command::revert::RevertArgs),
    #[command(subcommand, about = "Manage set of tracked repositories")]
    Remote(command::remote::RemoteCmds),
    #[command(about = "Open the repository in the browser")]
    Open(command::open::OpenArgs),
    #[command(about = "Manage repository configurations", alias = "cfg")]
    Config(command::config::ConfigArgs),
    #[command(about = "Manage the log of reference changes (e.g., HEAD, branches)")]
    Reflog(command::reflog::ReflogArgs),
    #[command(
        about = "Manage multiple working trees attached to this repository",
        alias = "wt"
    )]
    Worktree(command::worktree::WorktreeArgs),
    #[command(about = "Cloud backup and restore operations (D1/R2)")]
    Cloud(command::cloud::CloudArgs),

    // other hidden commands
    #[command(about = "Provide content, type or size info for repository objects")]
    CatFile(command::cat_file::CatFileArgs),

    #[command(
        about = "Build pack index file for an existing packed archive",
        hide = true
    )]
    IndexPack(command::index_pack::IndexPackArgs),

    #[command(
        about = "Check out and switch to a local or remote branches",
        hide = true
    )]
    Checkout(command::checkout::CheckoutArgs),
    #[command(
        subcommand,
        about = "Use binary search to find the commit that introduced a bug"
    )]
    Bisect(Bisect),
}

#[derive(Subcommand, Debug)]
pub enum Stash {
    #[command(about = "Save your local modifications to a new stash")]
    Push {
        #[arg(short, long, help = "The message to display for the stash")]
        message: Option<String>,
    },
    #[command(about = "Remove a single stashed state from the stash list")]
    Pop {
        #[arg(help = "The stash to pop")]
        stash: Option<String>,
    },
    #[command(about = "List the stashes that you currently have")]
    List,
    #[command(about = "Like pop, but do not remove the state from the stash list")]
    Apply {
        #[arg(help = "The stash to apply")]
        stash: Option<String>,
    },
    #[command(about = "Remove a single stashed state from the stash list")]
    Drop {
        #[arg(help = "The stash to drop")]
        stash: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum Bisect {
    #[command(about = "Start a new bisect session")]
    Start {
        #[arg(help = "Bad commit to start from")]
        bad: Option<String>,
        #[arg(long, short, help = "Good commit to mark")]
        good: Option<String>,
    },
    #[command(about = "Mark the current or given commit as bad")]
    Bad {
        #[arg(help = "Commit to mark as bad")]
        rev: Option<String>,
    },
    #[command(about = "Mark the current or given commit as good")]
    Good {
        #[arg(help = "Commit to mark as good")]
        rev: Option<String>,
    },
    #[command(about = "End bisect session and restore original HEAD")]
    Reset {
        #[arg(help = "Commit to reset to (optional)")]
        rev: Option<String>,
    },
    #[command(about = "Skip current commit and move to next")]
    Skip {
        #[arg(help = "Commit to skip")]
        rev: Option<String>,
    },
    #[command(about = "Show bisect log")]
    Log,
}

/// The main function is the entry point of the Libra application.
/// It parses the command-line arguments and executes the corresponding function.
/// - `args`: parse from command line if it's `None`, otherwise parse from the given args
pub fn parse(args: Option<&[&str]>) -> CliResult<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::fatal(format!("failed to create tokio runtime: {e}")))?;

    runtime.block_on(Box::pin(parse_async(args)))
}

// Rewrite `log -<n>` into `log -n <n>` only when `log` is the actual subcommand.
fn rewrite_log_short_number_args(args: Vec<String>) -> Vec<String> {
    // Detect the real subcommand position to avoid rewriting positional args for other commands.
    let subcommand = find_subcommand_index(&args);
    let Some((log_index, from_double_dash)) = subcommand else {
        return args;
    };
    if !matches!(args.get(log_index), Some(name) if name == "log") {
        return args;
    }

    let mut out: Vec<String> = Vec::with_capacity(args.len() + 2);
    if from_double_dash {
        // Drop the `--` that was used to separate global args from the subcommand.
        for (idx, arg) in args.iter().enumerate().take(log_index + 1) {
            if idx + 1 == log_index && arg == "--" {
                continue;
            }
            out.push(arg.clone());
        }
    } else {
        out.extend(args.iter().take(log_index + 1).cloned());
    }

    // Respect `--` inside the log subcommand: stop rewriting after it.
    let mut after_double_dash = false;
    for arg in args.into_iter().skip(log_index + 1) {
        if after_double_dash {
            out.push(arg);
            continue;
        }

        if arg == "--" {
            after_double_dash = true;
            out.push(arg);
            continue;
        }

        if is_short_number_flag(&arg) {
            out.push("-n".to_string());
            out.push(arg[1..].to_string());
        } else {
            out.push(arg);
        }
    }

    out
}

// Find the first argument that represents the subcommand.
// If `--` appears, treat the next argument as the subcommand.
fn find_subcommand_index(args: &[String]) -> Option<(usize, bool)> {
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            return if i + 1 < args.len() {
                Some((i + 1, true))
            } else {
                None
            };
        }
        if !arg.starts_with('-') {
            return Some((i, false));
        }
        i += 1;
    }
    None
}

fn is_short_number_flag(arg: &str) -> bool {
    if !arg.starts_with('-') || arg.len() < 2 {
        return false;
    }
    let rest = &arg[1..];
    rest.chars().all(|c| c.is_ascii_digit())
}

/// Inputs that look like top-level subcommands but should be redirected elsewhere.
/// Each entry is (input, hint_message).  Only needed for words that cannot be
/// expressed as a clap `alias` (e.g. they map to a *flag* of another command).
const REDIRECTED_COMMANDS: &[(&str, &str)] =
    &[("import", "You probably want `libra config --import`.")];

/// Build extra hint lines for an unrecognised-subcommand error.
///
/// The hints supplement (never duplicate) clap's built-in "tip: a similar
/// subcommand exists" message.  We only emit our own hints for cases that
/// clap cannot know about – e.g. redirecting `libra import` to
/// `libra config --import`.
fn parse_error_hints(err: &clap::Error) -> Vec<String> {
    let mut hints = Vec::new();

    if let Some(ContextValue::String(cmd)) = err.get(ContextKind::InvalidSubcommand) {
        let cmd_lower = cmd.to_lowercase();

        // Check redirected commands (e.g. `libra import` → `libra config --import`).
        for &(input, message) in REDIRECTED_COMMANDS {
            if cmd_lower == input {
                hints.push(message.to_string());
            }
        }
    }
    hints
}

fn parse_error_components(err: &clap::Error) -> (String, Option<String>, Vec<String>) {
    let rendered = err.to_string();
    let mut message = None;
    let mut usage_lines = Vec::new();
    let mut hints = Vec::new();

    for line in rendered.lines() {
        let trimmed = line.trim_start();
        if let Some(tip) = trimmed.strip_prefix("tip:") {
            hints.push(tip.trim().to_string());
            continue;
        }
        if message.is_none() {
            if let Some(msg) = trimmed.strip_prefix("error:") {
                message = Some(msg.trim().to_string());
                continue;
            }
            if !trimmed.is_empty() {
                message = Some(trimmed.to_string());
                continue;
            }
        }
        usage_lines.push(line.to_string());
    }

    hints.extend(parse_error_hints(err));

    let usage = if usage_lines.is_empty() {
        None
    } else {
        Some(usage_lines.join("\n").trim().to_string())
    };

    (
        message.unwrap_or_else(|| rendered.trim().to_string()),
        usage,
        hints,
    )
}

fn repo_not_found_error() -> CliError {
    CliError::repo_not_found()
}

fn is_error_codes_help_topic(argv: &[String]) -> bool {
    let Some((index, _)) = find_subcommand_index(argv) else {
        return false;
    };
    if !matches!(argv.get(index).map(String::as_str), Some("help")) {
        return false;
    }
    if !matches!(
        argv.get(index + 1).map(String::as_str),
        Some("error-codes" | "errors")
    ) {
        return false;
    }
    index + 2 == argv.len()
}

fn print_error_codes_help() -> CliResult<()> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(ERROR_CODES_HELP.as_bytes())
        .map_err(|e| CliError::fatal(format!("failed to write error code help: {e}")))?;
    stdout
        .flush()
        .map_err(|e| CliError::fatal(format!("failed to flush error code help: {e}")))?;
    Ok(())
}

fn is_top_level_unknown_command(argv: &[String], err: &clap::Error) -> Option<String> {
    let invalid = match err.get(ContextKind::InvalidSubcommand) {
        Some(ContextValue::String(cmd)) => cmd,
        _ => return None,
    };

    let (index, _) = find_subcommand_index(argv)?;
    if argv.get(index).is_some_and(|arg| arg == invalid) {
        return Some(invalid.to_string());
    }

    None
}

fn classify_parse_error(argv: &[String], err: &clap::Error) -> CliError {
    if let Some(cmd) = is_top_level_unknown_command(argv, err) {
        let (_, _, hints) = parse_error_components(err);
        let mut cli_error = CliError::unknown_command(format!(
            "libra: '{cmd}' is not a libra command. See 'libra --help'."
        ));
        for hint in hints {
            cli_error = cli_error.with_hint(hint);
        }
        return cli_error;
    }

    let (message, usage, hints) = parse_error_components(err);
    let mut cli_error = if find_subcommand_index(argv).is_some() {
        match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => CliError::parse_usage(message),
            _ => CliError::command_usage(message),
        }
    } else {
        CliError::parse_usage(message)
    };

    if let Some(usage) = usage {
        cli_error = cli_error.with_usage(usage);
    }
    for hint in hints {
        cli_error = cli_error.with_hint(hint);
    }

    cli_error
}

/// `async` version of the [parse] function
pub async fn parse_async(args: Option<&[&str]>) -> CliResult<()> {
    let argv = match args {
        Some(args) => args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        None => env::args().collect::<Vec<_>>(),
    };
    let argv = rewrite_log_short_number_args(argv);
    utils::output::reset_warning_tracker();
    if is_error_codes_help_topic(&argv) {
        return print_error_codes_help();
    }
    let args = match Cli::try_parse_from(argv.clone()) {
        Ok(args) => args,
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                err.print().map_err(|print_err| {
                    CliError::fatal(format!("failed to write clap output: {print_err}"))
                })?;
                return Ok(());
            }
            _ => return Err(classify_parse_error(&argv, &err)),
        },
    };
    if let Commands::Tag(tag_args) = &args.command {
        command::tag::validate_cli_args(tag_args)?;
    }
    match &args.command {
        Commands::Init(_) | Commands::Clone(_) | Commands::Open(_) => {}
        // Config global/system scopes don't require a repository
        Commands::Config(cfg) if cfg.global || cfg.system => {}
        _ => {
            let storage =
                utils::util::try_get_storage_path(None).map_err(|_| repo_not_found_error())?;
            set_local_hash_kind_for_storage(&storage).await?;
        }
    }
    // Resolve global output flags into a single config before dispatching.
    let output = OutputConfig::resolve(
        args.json.as_deref(),
        args.machine,
        args.no_pager,
        &args.color,
        args.quiet,
        args.exit_code_on_warning,
        &args.progress,
    );
    output.apply_color_override();

    // parse the command and execute the corresponding function with it's args
    match args.command {
        Commands::Init(cmd_args) => {
            let original_dir = utils::util::cur_dir();
            let init_target = if Path::new(&cmd_args.repo_directory).is_absolute() {
                Path::new(&cmd_args.repo_directory).to_path_buf()
            } else {
                original_dir.join(&cmd_args.repo_directory)
            };
            let storage = if cmd_args.bare {
                init_target
            } else {
                init_target.join(utils::util::ROOT_DIR)
            };

            command::init::execute_safe(cmd_args, &output).await?;
            set_local_hash_kind_for_storage(&storage).await?;
            env::set_current_dir(&original_dir).map_err(|e| {
                CliError::fatal(format!(
                    "failed to restore working directory '{}': {}",
                    original_dir.display(),
                    e
                ))
            })?;
        }
        Commands::Clone(cmd_args) => command::clone::execute_safe(cmd_args, &output).await?,
        Commands::Code(cmd_args) => command::code::execute(cmd_args, &output).await?,
        Commands::Add(cmd_args) => command::add::execute_safe(cmd_args, &output).await?,
        Commands::Rm(cmd_args) => command::remove::execute_safe(cmd_args, &output).await?,
        Commands::Restore(cmd_args) => command::restore::execute_safe(cmd_args, &output).await?,
        Commands::Status(cmd_args) => command::status::execute_safe(cmd_args, &output).await?,
        Commands::Clean(cmd_args) => command::clean::execute_safe(cmd_args, &output).await?,
        Commands::Stash(cmd) => command::stash::execute_safe(cmd, &output).await?,
        Commands::Lfs(cmd) => command::lfs::execute_safe(cmd, &output).await?,
        Commands::Log(cmd_args) => command::log::execute_safe(cmd_args, &output).await?,
        Commands::Shortlog(cmd_args) => command::shortlog::execute_safe(cmd_args, &output).await?,
        Commands::Show(cmd_args) => command::show::execute_safe(cmd_args, &output).await?,
        Commands::ShowRef(cmd_args) => command::show_ref::execute_safe(cmd_args, &output).await?,
        Commands::Branch(cmd_args) => command::branch::execute_safe(cmd_args, &output).await?,
        Commands::Tag(cmd_args) => command::tag::execute_safe(cmd_args, &output).await?,
        Commands::Commit(cmd_args) => command::commit::execute_safe(cmd_args, &output).await?,
        Commands::Switch(cmd_args) => command::switch::execute_safe(cmd_args, &output).await?,
        Commands::Rebase(cmd_args) => command::rebase::execute_safe(cmd_args, &output).await?,
        Commands::Merge(cmd_args) => command::merge::execute_safe(cmd_args, &output).await?,
        Commands::Reset(cmd_args) => command::reset::execute_safe(cmd_args, &output).await?,
        Commands::Mv(cmd_args) => command::mv::execute_safe(cmd_args, &output).await?,
        Commands::Describe(cmd_args) => command::describe::execute_safe(cmd_args, &output).await?,
        Commands::CherryPick(cmd_args) => {
            command::cherry_pick::execute_safe(cmd_args, &output).await?
        }
        Commands::Push(cmd_args) => command::push::execute_safe(cmd_args, &output).await?,
        Commands::CatFile(cmd_args) => command::cat_file::execute_safe(cmd_args, &output).await?,
        Commands::IndexPack(cmd_args) => command::index_pack::execute_safe(cmd_args, &output)?,
        Commands::Fetch(cmd_args) => command::fetch::execute_safe(cmd_args, &output).await?,
        Commands::Diff(cmd_args) => command::diff::execute_safe(cmd_args, &output).await?,
        Commands::Grep(cmd_args) => command::grep::execute_safe(cmd_args, &output).await?,
        Commands::Blame(cmd_args) => command::blame::execute_safe(cmd_args, &output).await?,
        Commands::Revert(cmd_args) => command::revert::execute_safe(cmd_args, &output).await?,
        Commands::Remote(cmd) => command::remote::execute_safe(cmd, &output).await?,
        Commands::Open(cmd_args) => command::open::execute_safe(cmd_args, &output).await?,
        Commands::Pull(cmd_args) => command::pull::execute_safe(cmd_args, &output).await?,
        Commands::Config(cmd_args) => command::config::execute_safe(cmd_args, &output).await?,
        Commands::Checkout(cmd_args) => command::checkout::execute_safe(cmd_args, &output).await?,
        Commands::Reflog(cmd_args) => command::reflog::execute_safe(cmd_args, &output).await?,
        Commands::Worktree(cmd_args) => command::worktree::execute_safe(cmd_args, &output).await?,
        Commands::Cloud(cmd_args) => command::cloud::execute_safe(cmd_args, &output).await?,
        Commands::Bisect(bisect_cmd) => command::bisect::execute_safe(bisect_cmd, &output).await?,
    }

    // Check for warnings when --exit-code-on-warning is active.
    if output.exit_code_on_warning && utils::output::warning_was_emitted() {
        return Err(CliError::failure("command completed with warnings")
            .with_stable_code(utils::error::StableErrorCode::WarningEmitted));
    }

    // Wait for any background storage tasks (e.g. object indexing) to complete
    // This prevents tasks from being killed when the process exits
    let _ = tokio::task::spawn_blocking(|| {
        utils::client_storage::ClientStorage::wait_for_background_tasks();
    })
    .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;
    use crate::utils::output;

    /// this test is to verify that the CLI can be built without panicking
    /// according [clap dock](https://docs.rs/clap/latest/clap/_derive/_tutorial/chapter_4/index.html)
    #[test]
    fn verify_cli() {
        use clap::CommandFactory;

        Cli::command().debug_assert()
    }

    #[tokio::test]
    async fn parse_error_shows_import_hint() {
        let err = parse_async(Some(&["libra", "import"])).await.unwrap_err();
        let msg = err.render();
        assert!(
            msg.contains("You probably want `libra config --import`."),
            "got: {msg}"
        );
    }

    #[test]
    fn clap_alias_br_resolves_to_branch() {
        let cli = Cli::try_parse_from(["libra", "br"]).unwrap();
        assert!(
            matches!(cli.command, Commands::Branch(_)),
            "`br` should parse as the branch subcommand"
        );
    }

    #[test]
    fn clap_alias_cfg_resolves_to_config() {
        let cli = Cli::try_parse_from(["libra", "cfg"]).unwrap();
        assert!(
            matches!(cli.command, Commands::Config(_)),
            "`cfg` should parse as the config subcommand"
        );
    }

    #[tokio::test]
    async fn clap_fuzzy_suggests_similar_command() {
        // "initt" is close enough to "init" for clap's built-in fuzzy match.
        let err = parse_async(Some(&["libra", "initt"])).await.unwrap_err();
        let msg = err.render();
        // Clap should include its own "tip: a similar subcommand exists: 'init'".
        assert!(
            msg.contains("Hint:") || msg.contains("similar"),
            "expected clap fuzzy-match suggestion, got: {msg}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn parse_async_resets_warning_tracker_before_dispatch() {
        output::record_warning();
        assert!(output::warning_was_emitted());

        parse_async(Some(&["libra", "--help"])).await.unwrap();

        assert!(
            !output::warning_was_emitted(),
            "top-level CLI dispatch should clear stale warning state before running"
        );
    }

    #[test]
    fn detects_help_error_codes_topic() {
        assert!(is_error_codes_help_topic(&[
            "libra".to_string(),
            "help".to_string(),
            "error-codes".to_string(),
        ]));
        assert!(is_error_codes_help_topic(&[
            "libra".to_string(),
            "help".to_string(),
            "errors".to_string(),
        ]));
        assert!(!is_error_codes_help_topic(&[
            "libra".to_string(),
            "help".to_string(),
            "status".to_string(),
        ]));
        assert!(!is_error_codes_help_topic(&[
            "libra".to_string(),
            "--help".to_string(),
        ]));
    }
}
