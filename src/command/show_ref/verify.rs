use std::{io::Write, str::FromStr};

use git_internal::hash::ObjectHash;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use super::{
    ShowRefArgs, ShowRefEntry, exists::remote_tracking_parts, show_ref_branch_store_error,
    tag_entries,
};
use crate::{
    internal::{branch::Branch, db::get_db_conn_instance, head::Head, model::reference},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
    },
};

pub(super) async fn show_ref_verify(args: &ShowRefArgs, output: &OutputConfig) -> CliResult<()> {
    if args.pattern.is_empty() {
        return Err(CliError::fatal("--verify requires a reference").with_exit_code(128));
    }

    let mut entries = Vec::with_capacity(args.pattern.len());
    for target in &args.pattern {
        let Some(mut target_entries) = lookup_exact_ref(target, args.dereference).await? else {
            return verify_missing(target, output);
        };
        entries.append(&mut target_entries);
    }

    emit_verified_entries(&entries, args.hash, output)
}

async fn lookup_exact_ref(target: &str, dereference: bool) -> CliResult<Option<Vec<ShowRefEntry>>> {
    if target == "HEAD" {
        return head_entry().await;
    }

    if let Some(branch_name) = target.strip_prefix("refs/heads/") {
        return branch_entry(branch_name, None, target).await;
    }

    if let Some((remote, branch_name)) = remote_tracking_parts(target) {
        if let Some(entry) = branch_entry(branch_name, Some(remote), target).await? {
            return Ok(Some(entry));
        }
        return branch_entry(target, Some(remote), target).await;
    }

    if target.strip_prefix("refs/tags/").is_some() {
        return tag_entry(target, dereference).await;
    }

    Ok(None)
}

async fn head_entry() -> CliResult<Option<Vec<ShowRefEntry>>> {
    Head::current_commit_result()
        .await
        .map(|maybe_hash| {
            maybe_hash.map(|hash| {
                vec![ShowRefEntry {
                    hash: hash.to_string(),
                    refname: "HEAD".to_string(),
                }]
            })
        })
        .map_err(|error| show_ref_branch_store_error("resolve HEAD", error))
}

async fn branch_entry(
    branch_name: &str,
    remote: Option<&str>,
    refname: &str,
) -> CliResult<Option<Vec<ShowRefEntry>>> {
    Branch::find_branch_result(branch_name, remote)
        .await
        .map(|maybe_branch| {
            maybe_branch.map(|branch| {
                vec![ShowRefEntry {
                    hash: branch.commit.to_string(),
                    refname: refname.to_string(),
                }]
            })
        })
        .map_err(|error| show_ref_branch_store_error("resolve branch reference", error))
}

async fn tag_entry(refname: &str, dereference: bool) -> CliResult<Option<Vec<ShowRefEntry>>> {
    let db = get_db_conn_instance().await;
    let row = reference::Entity::find()
        .filter(reference::Column::Name.eq(refname))
        .filter(reference::Column::Kind.eq(reference::ConfigKind::Tag))
        .filter(reference::Column::Remote.is_null())
        .one(&db)
        .await
        .map_err(|error| {
            CliError::fatal(format!(
                "failed to query tag reference '{refname}': {error}"
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;

    let Some(row) = row else {
        return Ok(None);
    };
    let Some(hash) = row.commit else {
        return Err(corrupt_tag_ref(refname, "missing object hash"));
    };
    let target =
        ObjectHash::from_str(&hash).map_err(|error| corrupt_tag_ref(refname, error.to_string()))?;

    tag_entries::entries_for_tag_target(refname, &target, dereference)
        .await
        .map(Some)
}

fn verify_missing(target: &str, output: &OutputConfig) -> CliResult<()> {
    if output.quiet {
        Err(CliError::silent_exit(1))
    } else {
        Err(CliError::fatal(format!("'{target}' - not a valid ref")).with_exit_code(128))
    }
}

fn emit_verified_entries(
    entries: &[ShowRefEntry],
    hash_only: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data(
            "show-ref",
            &serde_json::json!({
                "hash_only": hash_only,
                "entries": entries,
            }),
            output,
        );
    }

    if output.quiet {
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    for entry in entries {
        if hash_only {
            writeln!(writer, "{}", entry.hash).map_err(|error| {
                CliError::io(format!("failed to write show-ref output: {error}"))
            })?;
        } else {
            writeln!(writer, "{} {}", entry.hash, entry.refname).map_err(|error| {
                CliError::io(format!("failed to write show-ref output: {error}"))
            })?;
        }
    }
    Ok(())
}

fn corrupt_tag_ref(refname: &str, detail: impl std::fmt::Display) -> CliError {
    CliError::fatal(format!(
        "stored tag reference '{refname}' is corrupt: {detail}"
    ))
    .with_stable_code(StableErrorCode::RepoCorrupt)
}
