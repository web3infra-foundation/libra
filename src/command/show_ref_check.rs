use crate::{
    command::{
        show_ref::{ShowRefArgs, collect_raw_show_ref_entries},
        show_ref_render::{ShowRefRenderOptions, render_show_ref_entries},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

pub(crate) async fn execute_verify(args: &ShowRefArgs, output: &OutputConfig) -> CliResult<()> {
    if args.pattern.is_empty() {
        return Err(CliError::command_usage("--verify requires a reference").with_exit_code(128));
    }

    let entries = collect_raw_show_ref_entries(true, true, true, args.dereference).await?;
    let mut verified = Vec::new();
    for refname in &args.pattern {
        let Some(entry) = entries.iter().find(|entry| entry.refname == *refname) else {
            let exit_code = if output.quiet { 1 } else { 128 };
            return Err(CliError::failure(format!("'{refname}' - not a valid ref"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_exit_code(exit_code));
        };
        verified.push(entry.clone());
    }

    render_show_ref_entries(
        &verified,
        ShowRefRenderOptions::from_args(args.hash, args.abbrev),
        output,
    )
}

pub(crate) async fn execute_exists(args: &ShowRefArgs, output: &OutputConfig) -> CliResult<()> {
    if args.pattern.len() != 1 {
        let message = if args.pattern.is_empty() {
            "--exists requires a reference"
        } else {
            "--exists requires exactly one reference"
        };
        return Err(CliError::command_usage(message).with_exit_code(128));
    }

    let refname = &args.pattern[0];
    let entries = collect_raw_show_ref_entries(true, true, true, false).await?;
    if !entries.iter().any(|entry| entry.refname == *refname) {
        return Err(
            CliError::failure(format!("reference does not exist: {refname}"))
                .with_stable_code(StableErrorCode::CliInvalidTarget)
                .with_exit_code(2),
        );
    }

    if !output.is_json() {
        return Ok(());
    }
    emit_json_data(
        "show-ref",
        &serde_json::json!({ "exists": true, "refname": refname }),
        output,
    )
}
