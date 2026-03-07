//! Pull command combining fetch with merge or rebase depending on options, handling fast-forward checks and remote tracking setup.

use clap::Parser;

use super::{fetch, merge};
use crate::{
    internal::{config::Config, head::Head},
    utils::error::{CliError, CliResult},
};
#[derive(Parser, Debug)]
pub struct PullArgs {
    /// The repository to pull from
    repository: Option<String>,

    /// The refspec to pull, usually a branch name
    #[clap(requires("repository"))]
    refspec: Option<String>,
}

impl PullArgs {
    pub fn make(repository: Option<String>, refspec: Option<String>) -> Self {
        Self {
            repository,
            refspec,
        }
    }
}

pub async fn execute(args: PullArgs) {
    if let Err(err) = execute_safe(args).await {
        eprintln!("{}", err.render());
    }
}

pub async fn execute_safe(args: PullArgs) -> CliResult<()> {
    let fetch_args = fetch::FetchArgs {
        repository: args.repository.clone(),
        refspec: args.refspec.clone(),
        all: false,
    };
    fetch::execute_safe(fetch_args).await?;

    if let (Some(remote), Some(refspec)) = (&args.repository, &args.refspec) {
        merge::execute(merge::MergeArgs {
            branch: format!("{remote}/{refspec}"),
        })
        .await;
        return Ok(());
    }

    let head = Head::current().await;
    match head {
        Head::Branch(name) => match Config::branch_config(&name).await {
            Some(branch_config) => {
                let merge_args = merge::MergeArgs {
                    branch: format!("{}/{}", branch_config.remote, branch_config.merge),
                };
                merge::execute(merge_args).await;
                Ok(())
            }
            None => Err(CliError::failure(
                "There is no tracking information for the current branch.",
            )
            .with_hint("Run 'libra branch --set-upstream-to=<remote>/<branch>' to track a branch.")
            .with_hint(
                "Or specify a remote and branch, for example 'libra pull <remote> <branch>'.",
            )),
        },
        _ => Err(CliError::failure("You are not currently on a branch.")),
    }
}
