//! CLI entry for Libra.
//!
//! Defines the clap subcommand grammar, performs cross-cutting preflight (locating the
//! repository database and pinning the global hash algorithm to whatever is recorded
//! in `core.objectformat`), and dispatches every parsed command to its `command::*`
//! handler.
//!
//! Because every subcommand reads or writes objects whose hash kind must match the
//! repository's recorded `core.objectformat`, this module is the single point where
//! that global is configured before any handler runs.

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

/// Read the repository's `core.objectformat` and pin the global hash algorithm.
///
/// Functional scope:
/// - Opens the SQLite database at `<storage>/<DATABASE>` and reads
///   `core.objectformat`, defaulting to `"sha1"` when the row is absent.
/// - Calls `git_internal::hash::set_hash_kind` so every object hashed by the rest of
///   the process matches the repository's storage format.
///
/// Boundary conditions:
/// - Returns a fatal error when the database file is missing — every non-`init`,
///   non-`clone` command requires a repository, and silently continuing would hash
///   objects with the wrong algorithm.
/// - Returns a fatal error when the database cannot be opened (permissions, disk
///   corruption) so the user sees the underlying message instead of a downstream
///   panic.
/// - Currently accepts only `"sha1"` and `"sha256"`; anything else is rejected with a
///   fatal error.
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
    #[command(about = "Drive a local Libra Code TUI automation control session")]
    CodeControl(command::code_control::CodeControlArgs),
    #[command(about = "Inspect an AI thread version graph in a TUI")]
    Graph(command::graph::GraphArgs),
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
    #[command(about = "Parse and normalize revision names and repository paths")]
    RevParse(command::rev_parse::RevParseArgs),
    #[command(about = "List commit objects reachable from a revision")]
    RevList(command::rev_list::RevListArgs),
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

/// Synchronous CLI entry — used by both the `libra` binary and embedders that cannot
/// (or do not wish to) own their own Tokio runtime.
///
/// Functional scope:
/// - Builds a multi-thread Tokio runtime, then drives [`parse_async`] to completion.
/// - When `args` is `None`, the underlying parser falls back to `std::env::args`.
///
/// Boundary conditions:
/// - Calling this from inside an existing Tokio runtime panics; embedders that are
///   already async must call [`parse_async`] directly. See the embedding contract in
///   [`crate::exec`].
/// - Returns `CliError::fatal` if the runtime itself cannot be constructed (extremely
///   unlikely outside of OOM scenarios).
pub fn parse(args: Option<&[&str]>) -> CliResult<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::fatal(format!("failed to create tokio runtime: {e}")))?;

    runtime.block_on(Box::pin(parse_async(args)))
}

/// Rewrite Git-style `-<n>` shortcuts into the long-form `-n <n>` flag, but only when
/// the active subcommand is `log`.
///
/// Git accepts `git log -3` as shorthand for `git log -n 3`, but clap cannot express a
/// purely numeric flag without conflicting with positional revisions. This helper
/// patches argv before clap sees it so users keep the familiar shortcut.
///
/// Boundary conditions:
/// - The rewrite only fires for arguments before any `--` separator inside the `log`
///   subcommand, so paths or revisions that happen to look like `-3` are preserved
///   verbatim once the user explicitly closes the option list.
/// - When `log` is not the active subcommand the original argv is returned unchanged,
///   leaving every other command's `-<n>` semantics untouched.
///
/// See: [`tests::clap_alias_br_resolves_to_branch`] and friends for related parser
/// behaviour. The exact rewrite is exercised end-to-end by the integration tests in
/// `tests/command/log_test.rs`.
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

/// Locate the first non-flag token in `args` and return its index plus whether it was
/// produced by an explicit `--` separator.
///
/// Boundary conditions:
/// - Skips over any leading flags (`-x`, `--long`) so `libra --json status` still
///   identifies `status` as the subcommand.
/// - When `--` appears, the *next* argument is treated as the subcommand and the
///   returned `bool` is `true` to signal the caller to drop the separator. Returns
///   `None` if `--` is the last token.
/// - Returns `None` when no non-flag token exists (e.g. argv is `["libra"]` or
///   `["libra", "--help"]`).
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

const REMOVED_CODE_CLAUDECODE_FLAGS: &[&str] = &[
    "--resume-session",
    "--fork-session",
    "--session-id",
    "--resume-at",
    "--helper-path",
    "--python-binary",
    "--timeout-seconds",
    "--permission-mode",
];

fn removed_code_claudecode_hints(argv: &[String]) -> Vec<String> {
    let Some((subcommand_index, _)) = find_subcommand_index(argv) else {
        return Vec::new();
    };
    if !matches!(argv.get(subcommand_index).map(String::as_str), Some("code")) {
        return Vec::new();
    }

    let mut hints = Vec::new();
    let has_removed_provider = argv.windows(2).any(
        |window| matches!(window, [flag, value] if flag == "--provider" && value == "claudecode"),
    ) || argv.iter().any(|arg| arg == "--provider=claudecode");
    if has_removed_provider {
        hints.push(
            "`libra code --provider claudecode` was removed; use `--provider codex` for the managed agent runtime or `--provider anthropic` for direct Anthropic chat completions."
                .to_string(),
        );
    }

    let has_removed_flag = argv.iter().any(|arg| {
        REMOVED_CODE_CLAUDECODE_FLAGS
            .iter()
            .any(|flag| arg == flag || arg.starts_with(&format!("{flag}=")))
    });
    if has_removed_flag {
        hints.push(
            "Claude Code provider-session flags were removed with the managed runtime; start a new Codex or generic-provider session and use Libra's canonical `--resume <thread_id>` flow."
                .to_string(),
        );
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

fn shell_quote_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    shell_quote_text(&raw)
}

#[cfg(windows)]
fn shell_quote_text(raw: &str) -> String {
    format!("\"{}\"", raw)
}

#[cfg(not(windows))]
fn shell_quote_text(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\"'\"'"))
}

fn git_conversion_hint(location: &utils::util::GitRepositoryLocation) -> String {
    let command = if location.is_bare {
        "libra init --bare --from-git-repository ."
    } else {
        "libra init --from-git-repository ."
    };
    let current = utils::util::cur_dir()
        .canonicalize()
        .unwrap_or_else(|_| utils::util::cur_dir());

    if current == location.root {
        format!("run '{command}' to convert this Git repository to Libra.")
    } else {
        format!("run: cd {} && {command}", shell_quote_path(&location.root))
    }
}

fn repo_not_found_error(path: Option<&Path>) -> CliError {
    let mut error = CliError::repo_not_found();
    if let Some(location) = utils::util::find_git_repository(path) {
        error = error.with_priority_hint(git_conversion_hint(&location));
    }
    error
}

fn command_preflight_storage(command: &Commands) -> CliResult<Option<std::path::PathBuf>> {
    match command {
        Commands::Init(_) | Commands::Clone(_) | Commands::Open(_) | Commands::CodeControl(_) => {
            Ok(None)
        }
        #[cfg(unix)]
        Commands::Worktree(command::worktree::WorktreeArgs {
            command: command::worktree::WorktreeSubcommand::Umount { .. },
        }) => Ok(None),
        // Config global/system scopes don't require a repository.
        Commands::Config(cfg) if cfg.global || cfg.system => Ok(None),
        Commands::Code(code_args) => {
            let working_dir = command::code::resolve_code_preflight_working_dir(code_args)?;
            let storage = utils::util::try_get_storage_path(Some(working_dir.clone()))
                .map_err(|_| repo_not_found_error(Some(&working_dir)))?;
            Ok(Some(storage))
        }
        Commands::Graph(graph_args) => {
            let storage = utils::util::try_get_storage_path(graph_args.repo.clone())
                .map_err(|_| repo_not_found_error(graph_args.repo.as_deref()))?;
            Ok(Some(storage))
        }
        _ => {
            let storage =
                utils::util::try_get_storage_path(None).map_err(|_| repo_not_found_error(None))?;
            Ok(Some(storage))
        }
    }
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

    let (message, usage, mut hints) = parse_error_components(err);
    hints.extend(removed_code_claudecode_hints(argv));
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

/// Async CLI dispatcher — the actual orchestrator behind every Libra invocation.
///
/// Functional scope:
/// 1. Normalises argv (rewrites `log -<n>` shortcuts, strips a leading `--`).
/// 2. Resets the per-process warning tracker so `--exit-code-on-warning` cannot be
///    polluted by a previous invocation in long-lived processes (TUI, tests).
/// 3. Short-circuits the `help error-codes` topic before clap parsing because it
///    would otherwise be treated as an unknown subcommand.
/// 4. Parses with clap and translates every parse failure into a structured
///    [`CliError`] (see [`classify_parse_error`]).
/// 5. Validates command-specific arg constraints that clap cannot express (e.g.
///    [`command::tag::validate_cli_args`]).
/// 6. For commands that operate on a repository, runs [`command_preflight_storage`]
///    and primes the global hash kind via [`set_local_hash_kind_for_storage`].
/// 7. Resolves the global output flags into a single [`OutputConfig`] and dispatches
///    to the matching `command::*::execute_safe` handler.
/// 8. After the command returns, waits for any background storage tasks (object
///    indexing, cache flushes) so they cannot be killed by process exit.
///
/// Boundary conditions:
/// - `--help` / `--version` are still rendered through clap so output matches user
///   expectations exactly; the function then returns `Ok(())` without dispatching.
/// - The `Init` arm explicitly restores the original CWD afterwards because the
///   handler may `cd` into a freshly-created repo and downstream callers (notably
///   the integration test suite and `--from-git-repository`) rely on the CWD being
///   stable across invocations.
/// - When `--exit-code-on-warning` is set and at least one warning was recorded, the
///   function returns a `CliError::failure` with stable code `WarningEmitted` even
///   though the underlying command succeeded.
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
    if let Some(storage) = command_preflight_storage(&args.command)? {
        set_local_hash_kind_for_storage(&storage).await?;
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
        Commands::CodeControl(cmd_args) => command::code_control::execute(cmd_args).await?,
        Commands::Graph(cmd_args) => command::graph::execute_safe(cmd_args, &output).await?,
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
        Commands::RevParse(cmd_args) => command::rev_parse::execute_safe(cmd_args, &output).await?,
        Commands::RevList(cmd_args) => command::rev_list::execute_safe(cmd_args, &output).await?,
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
    use std::fs;

    use serial_test::serial;

    use super::*;
    use crate::utils::{output, test};

    /// Scenario: clap's `debug_assert` walks the entire command tree and panics on any
    /// structural mistake (duplicate flags, conflicting aliases, malformed value
    /// parsers). This test is the cheapest way to keep the giant `Commands` enum
    /// honest as new subcommands are added.
    /// See: <https://docs.rs/clap/latest/clap/_derive/_tutorial/chapter_4/index.html>
    #[test]
    fn verify_cli() {
        use clap::CommandFactory;

        Cli::command().debug_assert()
    }

    /// Scenario: `libra import` is intentionally not a subcommand because importing
    /// is exposed via `libra config --import`. This test guards the redirect hint
    /// emitted by [`parse_error_hints`] / [`REDIRECTED_COMMANDS`] so users typing the
    /// natural-but-wrong word are pointed at the real flag.
    #[tokio::test]
    async fn parse_error_shows_import_hint() {
        let err = parse_async(Some(&["libra", "import"])).await.unwrap_err();
        let msg = err.render();
        assert!(
            msg.contains("You probably want `libra config --import`."),
            "got: {msg}"
        );
    }

    /// Scenario: the `branch` command advertises a `br` alias for ergonomics. This
    /// test ensures the alias keeps resolving even after the `Commands` enum is
    /// reordered or extended.
    #[test]
    fn clap_alias_br_resolves_to_branch() {
        let cli = Cli::try_parse_from(["libra", "br"]).unwrap();
        assert!(
            matches!(cli.command, Commands::Branch(_)),
            "`br` should parse as the branch subcommand"
        );
    }

    /// Scenario: the `config` command advertises a `cfg` alias. Mirrors
    /// [`clap_alias_br_resolves_to_branch`] for the second alias that tends to break
    /// when the subcommand list is touched.
    #[test]
    fn clap_alias_cfg_resolves_to_config() {
        let cli = Cli::try_parse_from(["libra", "cfg"]).unwrap();
        assert!(
            matches!(cli.command, Commands::Config(_)),
            "`cfg` should parse as the config subcommand"
        );
    }

    #[cfg(unix)]
    #[test]
    fn worktree_umount_preflight_does_not_require_repo() {
        let cli = Cli::try_parse_from(["libra", "worktree", "umount", "/tmp/libra-task"]).unwrap();

        assert!(matches!(command_preflight_storage(&cli.command), Ok(None)));
    }

    /// Scenario: clap's built-in Levenshtein matcher should suggest `init` for the
    /// typo `initt`. We accept either "Hint:" (Libra-formatted) or "similar"
    /// (clap-formatted) so the test survives clap upgrades that re-word the message.
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

    /// Scenario: the warning tracker is a process-global static. In long-lived
    /// processes (TUI, tests) a previously-recorded warning would otherwise leak
    /// into the next invocation and silently flip the exit code under
    /// `--exit-code-on-warning`. This test seeds a stale warning, then verifies that
    /// [`parse_async`] clears it before any handler runs.
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

    /// Scenario: `libra code --repo <path>` should perform repository preflight
    /// against `<path>`, *not* the process CWD. The test arranges for the CWD to be
    /// outside any repo, sets `--repo` to a freshly-initialised one, and confirms
    /// that the error we hit is the *next* validation step (missing ollama model)
    /// rather than "not a libra repository". This guards a regression where preflight
    /// was hitting CWD before honoring `--repo`.
    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn code_repo_flag_uses_target_repo_during_preflight() {
        let root = tempfile::tempdir().expect("failed to create test root");
        let repo = root.path().join("linked");
        let outside = root.path().join("outside");
        fs::create_dir_all(&repo).expect("failed to create repo dir");
        fs::create_dir_all(&outside).expect("failed to create outside dir");
        test::setup_with_new_libra_in(&repo).await;

        let _guard = test::ChangeDirGuard::new(&outside);
        let repo_arg = repo
            .to_str()
            .expect("temporary repo path should be valid UTF-8");
        let err = parse_async(Some(&[
            "libra",
            "code",
            "--repo",
            repo_arg,
            "--provider",
            "ollama",
        ]))
        .await
        .expect_err("missing ollama model should stop after repository preflight");
        let rendered = err.render();

        assert!(
            rendered.contains("--model is required when using --provider ollama"),
            "expected provider validation after --repo preflight, got: {rendered}"
        );
        assert!(
            !rendered.contains("not a libra repository"),
            "--repo should be honored before checking the process cwd, got: {rendered}"
        );
    }

    /// Scenario: `libra help error-codes` (and its `errors` alias) should bypass
    /// clap and stream the bundled error-code reference. Tests cover the two valid
    /// spellings plus two negative cases — a different `help <topic>` and the global
    /// `--help` flag — so the matcher in [`is_error_codes_help_topic`] stays tight
    /// enough that we don't accidentally swallow other help requests.
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

    /// Scenario (Unix): paths embedded in conversion-hint messages must be
    /// shell-safe. POSIX shells require `'...'` quoting with `'\'\''` escapes for
    /// embedded single quotes; this test pins that rule using a path containing an
    /// apostrophe, the canonical breakage case.
    #[cfg(not(windows))]
    #[test]
    fn shell_quote_path_uses_posix_single_quote_escaping() {
        assert_eq!(
            shell_quote_path(Path::new("repo's path")),
            "'repo'\"'\"'s path'"
        );
    }

    /// Scenario (Windows): cmd.exe and PowerShell expect double-quoted paths and
    /// tolerate spaces inside them. This test pins the simpler Windows behaviour and
    /// exists as a sibling to the POSIX test so both platforms have explicit
    /// coverage when [`shell_quote_path`] is touched.
    #[cfg(windows)]
    #[test]
    fn shell_quote_path_uses_windows_double_quotes() {
        assert_eq!(
            shell_quote_path(Path::new(r"C:\Program Files\repo")),
            r#""C:\Program Files\repo""#
        );
    }
}
