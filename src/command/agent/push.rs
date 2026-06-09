//! `libra agent push [--remote <name>]` transport wrapper.
//!
//! The external-agent capture catalogue lives on the local
//! `agent-traces` branch, but the remote contract reserves
//! `refs/libra/agent-traces` so it does not appear as a user branch.

use super::PushArgs;
use crate::{
    command::push as push_command,
    internal::branch::AGENT_TRACES_BRANCH,
    utils::{error::CliResult, output::OutputConfig},
};

const DEFAULT_AGENT_TRACES_REMOTE: &str = "origin";
const AGENT_TRACES_REMOTE_REF: &str = "refs/libra/agent-traces";

pub async fn execute_safe(args: PushArgs, output: &OutputConfig) -> CliResult<()> {
    let remote = args
        .remote
        .unwrap_or_else(|| DEFAULT_AGENT_TRACES_REMOTE.to_string());
    let refspec = format!("{AGENT_TRACES_BRANCH}:{AGENT_TRACES_REMOTE_REF}");
    push_command::execute_safe(
        push_command::PushArgs::for_refspecs(remote, vec![refspec]),
        output,
    )
    .await
}
