//! `libra agent push [--remote <name>]` transport wrapper.
//!
//! The external-agent capture catalogue lives on the local
//! `traces` branch, but the remote contract reserves
//! `refs/libra/traces` so it does not appear as a user branch.

use super::PushArgs;
use crate::{
    command::push as push_command,
    internal::branch::TRACES_BRANCH,
    utils::{error::CliResult, output::OutputConfig},
};

const DEFAULT_TRACES_REMOTE: &str = "origin";
const TRACES_REMOTE_REF: &str = "refs/libra/traces";

pub async fn execute_safe(args: PushArgs, output: &OutputConfig) -> CliResult<()> {
    let remote = args
        .remote
        .unwrap_or_else(|| DEFAULT_TRACES_REMOTE.to_string());
    let refspec = format!("{TRACES_BRANCH}:{TRACES_REMOTE_REF}");
    push_command::execute_safe(
        push_command::PushArgs::for_refspecs(remote, vec![refspec]),
        output,
    )
    .await
}
