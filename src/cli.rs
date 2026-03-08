//! CLI entry for Libra, defining clap subcommands, setting the hash algorithm from config,
//! and dispatching each command handler.

use std::env;

use clap::{
    Parser, Subcommand,
    error::{ContextKind, ContextValue, ErrorKind},
};
use git_internal::hash::{HashKind, set_hash_kind};

use crate::{
    command, utils,
    utils::error::{CliError, CliResult},
};

/// Reads the repository's configuration and sets the global hash kind.
/// This must be called for any command that operates within an existing repository.
/// Returns an error if the repository database is missing or corrupted.
async fn set_local_hash_kind() -> CliResult<()> {
    // Verify the database file actually exists before accessing it, to avoid
    // panicking inside `get_db_conn_instance()` when `.libra` exists but
    // `libra.db` is missing (e.g. corrupted or partially-removed repo).
    let storage = utils::util::try_get_storage_path(None).map_err(|_| repo_not_found_error())?;
    let db_path = storage.join(utils::util::DATABASE);
    if !db_path.exists() {
        return Err(CliError::fatal(format!(
            "repository database not found at '{}'",
            db_path.display()
        )));
    }

    // Use the public API from the `config` module to get the configuration value.
    let object_format = crate::internal::config::Config::get("core", None, "objectformat")
        .await
        .unwrap_or_else(|| "sha1".to_string());

    let hash_kind = match object_format.as_str() {
        "sha1" => HashKind::Sha1,
        "sha256" => HashKind::Sha256,
        _ => {
            return Err(CliError::fatal(format!(
                "unsupported object format: '{}'",
                object_format
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
    version = "0.1.0"
)]
struct Cli {
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
    #[command(subcommand, about = "Handle Claude Code hook events")]
    ClaudeCode(command::claude_code::ClaudeCodeCommand),

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

/// The main function is the entry point of the Libra application.
/// It parses the command-line arguments and executes the corresponding function.
/// - Caution: This is a `synchronous` function, it's declared as `async` to be able to use `[tokio::main]`
/// - `args`: parse from command line if it's `None`, otherwise parse from the given args
#[tokio::main]
pub async fn parse(args: Option<&[&str]>) -> CliResult<()> {
    parse_async(args).await
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
            "libra: '{}' is not a libra command. See 'libra --help'.",
            cmd
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
    match &args.command {
        Commands::Init(_) | Commands::Clone(_) | Commands::ClaudeCode(_) => {}
        // Config global/system scopes don't require a repository
        Commands::Config(cfg) if cfg.global || cfg.system => {}
        _ => {
            utils::util::try_get_storage_path(None).map_err(|_| repo_not_found_error())?;
            set_local_hash_kind().await?;
        }
    }
    // parse the command and execute the corresponding function with it's args
    match args.command {
        Commands::Init(args) => {
            let original_dir = utils::util::cur_dir();
            command::init::execute_safe(args).await?; // set working directory as args.repo_directory
            set_local_hash_kind().await?; // set hash kind after init
            env::set_current_dir(&original_dir).map_err(|e| {
                CliError::fatal(format!(
                    "failed to restore working directory '{}': {}",
                    original_dir.display(),
                    e
                ))
            })?; // restore working directory as original_dir
        }
        Commands::Clone(args) => command::clone::execute_safe(args).await?, //clone will use init internally,so we don't need to set hash kind here again
        Commands::Code(args) => command::code::execute(args).await,
        Commands::ClaudeCode(cmd) => command::claude_code::execute(cmd)
            .await
            .map_err(|e| CliError::fatal(e.to_string()))?,
        Commands::Add(args) => command::add::execute_safe(args).await?,
        Commands::Rm(args) => command::remove::execute_safe(args).await?,
        Commands::Restore(args) => command::restore::execute_safe(args).await?,
        Commands::Status(args) => command::status::execute_safe(args).await?,
        Commands::Clean(args) => command::clean::execute_safe(args).await?,
        Commands::Stash(cmd) => command::stash::execute_safe(cmd).await?,
        Commands::Lfs(cmd) => command::lfs::execute_safe(cmd).await?,
        Commands::Log(args) => command::log::execute_safe(args).await?,
        Commands::Shortlog(args) => command::shortlog::execute_safe(args).await?,
        Commands::Show(args) => command::show::execute_safe(args).await?,
        Commands::ShowRef(args) => command::show_ref::execute_safe(args).await?,
        Commands::Branch(args) => command::branch::execute_safe(args).await?,
        Commands::Tag(args) => command::tag::execute_safe(args).await?,
        Commands::Commit(args) => command::commit::execute_safe(args).await?,
        Commands::Switch(args) => command::switch::execute_safe(args).await?,
        Commands::Rebase(args) => command::rebase::execute_safe(args).await?,
        Commands::Merge(args) => command::merge::execute_safe(args).await?,
        Commands::Reset(args) => command::reset::execute_safe(args).await?,
        Commands::Mv(args) => command::mv::execute_safe(args).await?,
        Commands::Describe(args) => command::describe::execute_safe(args).await?,
        Commands::CherryPick(args) => command::cherry_pick::execute_safe(args).await?,
        Commands::Push(args) => command::push::execute_safe(args).await?,
        Commands::CatFile(args) => command::cat_file::execute_safe(args).await?,
        Commands::IndexPack(args) => command::index_pack::execute_safe(args)?,
        Commands::Fetch(args) => command::fetch::execute_safe(args).await?,
        Commands::Diff(args) => command::diff::execute_safe(args).await?,
        Commands::Blame(args) => command::blame::execute_safe(args).await?,
        Commands::Revert(args) => command::revert::execute_safe(args).await?,
        Commands::Remote(cmd) => command::remote::execute_safe(cmd).await?,
        Commands::Open(args) => command::open::execute_safe(args).await?,
        Commands::Pull(args) => command::pull::execute_safe(args).await?,
        Commands::Config(args) => command::config::execute_safe(args).await?,
        Commands::Checkout(args) => command::checkout::execute_safe(args).await?,
        Commands::Reflog(args) => command::reflog::execute_safe(args).await?,
        Commands::Worktree(args) => command::worktree::execute_safe(args).await?,
        Commands::Cloud(args) => command::cloud::execute_safe(args).await?,
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
    use crate::utils::{error::CliErrorKind, test::ChangeDirGuard};

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

    #[tokio::test]
    #[serial]
    async fn clap_alias_br_resolves_to_branch() {
        // Run from a temp dir that has no `.libra` to guarantee RepoNotFound.
        let temp = tempfile::tempdir().unwrap();
        let _guard = ChangeDirGuard::new(temp.path());

        // `br` is a clap alias for `branch`, so it should NOT produce an error
        // but instead be dispatched like `libra branch` (which fails without a repo).
        let err = parse_async(Some(&["libra", "br"])).await.unwrap_err();
        // Should fail because no repo exists, not because the subcommand is unknown.
        assert!(
            !matches!(
                err.kind(),
                CliErrorKind::ParseUsage | CliErrorKind::CommandUsage
            ),
            "expected non-parse error (alias should resolve), got: {err:?}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn clap_alias_cfg_resolves_to_config() {
        // Run from a temp dir that has no `.libra` to guarantee RepoNotFound.
        let temp = tempfile::tempdir().unwrap();
        let _guard = ChangeDirGuard::new(temp.path());

        // `cfg` is a clap alias for `config`, dispatched normally.
        // Without arguments it should fail with a config validation error, not a parse error.
        let err = parse_async(Some(&["libra", "cfg"])).await.unwrap_err();
        assert!(
            !matches!(
                err.kind(),
                CliErrorKind::ParseUsage | CliErrorKind::CommandUsage
            ),
            "expected non-parse error (alias should resolve), got: {err:?}"
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
}
