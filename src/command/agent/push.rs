//! `libra agent push [--remote <name>]` — Phase 3 stub.
//!
//! The plan (`docs/improvement/entire.md` §10.3) wants this command to push
//! `refs/libra/agent-traces` to a configurable remote. The existing
//! `command::push` machinery only handles refspecs that map to
//! `refs/heads/<name>` on the remote (see `command/push.rs:509`), so a
//! "real" agent-traces push would either require refactoring `push.rs` to
//! accept fully qualified non-`heads/*` refspecs, or a dedicated transport
//! layer. Both belong to Phase 3.
//!
//! Until then, this command surfaces a clear diagnostic so users don't
//! accidentally publish the agent-traces tip to `refs/heads/agent-traces`
//! on a public remote (the previous draft of this wrapper would have done
//! exactly that — flagged by the Codex Phase-2-followups round-1 review).

use super::PushArgs;
use crate::utils::{
    error::{CliError, CliResult},
    output::OutputConfig,
};

pub async fn execute_safe(args: PushArgs, output: &OutputConfig) -> CliResult<()> {
    let remote_label = args.remote.as_deref().unwrap_or("(default)");
    if !output.quiet {
        eprintln!(
            "libra agent push: not yet implemented in phase 2 (target='{remote_label}'). \
             refs/libra/agent-traces transport requires phase 3 push refactoring; \
             see docs/improvement/entire.md §10.3."
        );
    }
    Err(CliError::fatal(
        "libra agent push not yet implemented (phase 3)".to_string(),
    ))
}
