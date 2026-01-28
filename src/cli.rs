//! CLI entry for Libra, defining clap subcommands, setting the hash algorithm from config, and dispatching each command handler.
use std::env;

use clap::{Parser, Subcommand};
use git_internal::{
    errors::GitError,
    hash::{HashKind, set_hash_kind},
};

use crate::{command, utils};
/// Reads the repository's configuration and sets the global hash kind.
/// This must be called for any command that operates within an existing repository.
async fn set_local_hash_kind() -> Result<(), GitError> {
    // Use the public API from the `config` module to get the configuration value.
    let object_format = crate::internal::config::Config::get("core", None, "objectformat")
        .await
        .unwrap_or_else(|| "sha1".to_string());

    let hash_kind = match object_format.as_str() {
        "sha1" => HashKind::Sha1,
        "sha256" => HashKind::Sha256,
        _ => {
            return Err(GitError::InvalidHashValue(format!(
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
    about = "Libra: A partial Git implemented in Rust",
    version = "0.1.0-pre"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// THe Commands enum represents the subcommands that can be used with the CLI.
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

    // The rest of the commands require a repository to be present
    #[command(about = "Add file contents to the index")]
    Add(command::add::AddArgs),
    #[command(about = "Remove files from the working tree and from the index")]
    Rm(command::remove::RemoveArgs),
    #[command(about = "Restore working tree files")]
    Restore(command::restore::RestoreArgs),
    #[command(about = "Show the working tree status")]
    Status(command::status::StatusArgs),
    #[command(
        subcommand,
        about = "Stash the changes in a dirty working directory away"
    )]
    Stash(Stash),
    #[command(subcommand, about = "Large File Storage")]
    Lfs(command::lfs::LfsCmds),
    #[command(about = "Show commit logs")]
    Log(command::log::LogArgs),
    #[command(about = "Show various types of objects")]
    Show(command::show::ShowArgs),
    #[command(about = "List, create, or delete branches")]
    Branch(command::branch::BranchArgs),
    #[command(about = "Create a new tag")]
    Tag(command::tag::TagArgs),
    #[command(about = "Record changes to the repository")]
    Commit(command::commit::CommitArgs),
    #[command(about = "Switch branches")]
    Switch(command::switch::SwitchArgs),
    #[command(about = "Reapply commits on top of another base tip")]
    Rebase(command::rebase::RebaseArgs),
    #[command(about = "Merge changes")]
    Merge(command::merge::MergeArgs),
    #[command(about = "Reset current HEAD to specified state")]
    Reset(command::reset::ResetArgs),
    #[command(about = "Apply the changes introduced by some existing commits")]
    CherryPick(command::cherry_pick::CherryPickArgs),
    #[command(about = "Update remote refs along with associated objects")]
    Push(command::push::PushArgs),
    #[command(about = "Download objects and refs from another repository")]
    Fetch(command::fetch::FetchArgs),
    #[command(about = "Fetch from and integrate with another repository or a local branch")]
    Pull(command::pull::PullArgs),
    #[command(about = "Show differences between files")]
    Diff(command::diff::DiffArgs),
    #[command(about = "Show author and history of each line of a file")]
    Blame(command::blame::BlameArgs),
    #[command(about = "Revert some existing commits")]
    Revert(command::revert::RevertArgs),
    #[command(subcommand, about = "Manage set of tracked repositories")]
    Remote(command::remote::RemoteCmds),
    #[command(about = "Open the repository in the browser")]
    Open(command::open::OpenArgs),
    #[command(about = "Manage repository configurations")]
    Config(command::config::ConfigArgs),
    #[command(about = "Manage the log of reference changes (e.g., HEAD, branches)")]
    Reflog(command::reflog::ReflogArgs),

    // other hidden commands
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
pub async fn parse(args: Option<&[&str]>) -> Result<(), GitError> {
    parse_async(args).await
}

/// `async` version of the [parse] function
pub async fn parse_async(args: Option<&[&str]>) -> Result<(), GitError> {
    let args = match args {
        Some(args) => {
            Cli::try_parse_from(args).map_err(|e| GitError::InvalidArgument(e.to_string()))?
        }
        None => Cli::parse(),
    };
    // TODO: try check repo before parsing
    // For commands that don't initialize a repo, set the hash kind first.
    match args.command {
        Commands::Init(_) | Commands::Clone(_) => {}
        _ => {
            if !utils::util::check_repo_exist() {
                return Err(GitError::RepoNotFound);
            }
            set_local_hash_kind().await?;
        }
    }
    // parse the command and execute the corresponding function with it's args
    match args.command {
        Commands::Init(args) => {
            let original_dir = utils::util::cur_dir();
            command::init::execute(args).await; // set working directory as args.repo_directory
            set_local_hash_kind().await?; // set hash kind after init
            env::set_current_dir(&original_dir)?; // restore working directory as original_dir
        }
        Commands::Clone(args) => command::clone::execute(args).await, //clone will use init internally,so we don't need to set hash kind here again
        Commands::Add(args) => command::add::execute(args).await,
        Commands::Rm(args) => command::remove::execute(args).await,
        Commands::Restore(args) => command::restore::execute(args).await,
        Commands::Status(args) => command::status::execute(args).await,
        Commands::Stash(cmd) => command::stash::execute(cmd).await,
        Commands::Lfs(cmd) => command::lfs::execute(cmd).await,
        Commands::Log(args) => command::log::execute(args).await,
        Commands::Show(args) => command::show::execute(args).await,
        Commands::Branch(args) => command::branch::execute(args).await,
        Commands::Tag(args) => command::tag::execute(args).await,
        Commands::Commit(args) => command::commit::execute(args).await,
        Commands::Switch(args) => command::switch::execute(args).await,
        Commands::Rebase(args) => command::rebase::execute(args).await,
        Commands::Merge(args) => command::merge::execute(args).await,
        Commands::Reset(args) => command::reset::execute(args).await,
        Commands::CherryPick(args) => command::cherry_pick::execute(args).await,
        Commands::Push(args) => command::push::execute(args).await,
        Commands::IndexPack(args) => command::index_pack::execute(args),
        Commands::Fetch(args) => command::fetch::execute(args).await,
        Commands::Diff(args) => command::diff::execute(args).await,
        Commands::Blame(args) => command::blame::execute(args).await,
        Commands::Revert(args) => command::revert::execute(args).await,
        Commands::Remote(cmd) => command::remote::execute(cmd).await,
        Commands::Open(args) => command::open::execute(args).await,
        Commands::Pull(args) => command::pull::execute(args).await,
        Commands::Config(args) => command::config::execute(args).await,
        Commands::Checkout(args) => command::checkout::execute(args).await,
        Commands::Reflog(args) => command::reflog::execute(args).await,
    }
    Ok(())
}

/// this test is to verify that the CLI can be built without panicking
/// according [clap dock](https://docs.rs/clap/latest/clap/_derive/_tutorial/chapter_4/index.html)
#[test]
fn verify_cli() {
    use clap::CommandFactory;

    Cli::command().debug_assert()
}
