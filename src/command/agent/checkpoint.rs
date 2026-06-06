//! `libra agent checkpoint …` subcommands. V1 ships read-only `list` /
//! `show`; `rewind --apply` restores the worktree and dispatches optional
//! transcript truncation for agent kinds that implement `TranscriptTruncator`.

use std::str::FromStr;

use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, tree::Tree},
};
use sea_orm::{ConnectionTrait, Statement};
use serde::Serialize;

use super::{CheckpointListArgs, CheckpointRewindArgs, CheckpointShowArgs, CheckpointSubcommand};
use crate::{
    command::load_object,
    internal::db::get_db_conn_instance,
    utils::{
        error::{CliError, CliResult},
        object::read_git_object,
        object_ext::TreeExt,
        output::{OutputConfig, emit_json_data},
        util,
    },
};

pub async fn execute_safe(cmd: CheckpointSubcommand, output: &OutputConfig) -> CliResult<()> {
    match cmd {
        CheckpointSubcommand::List(args) => list(args, output).await,
        CheckpointSubcommand::Show(args) => show(args, output).await,
        CheckpointSubcommand::Rewind(args) => rewind(args, output).await,
    }
}

#[derive(Debug, Serialize)]
struct CheckpointRow {
    checkpoint_id: String,
    session_id: String,
    scope: String,
    /// Nullable in the schema since the `2026050501` follow-up — stays
    /// `Option<String>` end-to-end so JSON consumers can distinguish a
    /// missing parent from an empty string.
    parent_commit: Option<String>,
    tree_oid: String,
    metadata_blob_oid: String,
    traces_commit: String,
    created_at: i64,
}

async fn list(args: CheckpointListArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    if !table_exists(&conn, "agent_checkpoint").await? {
        return emit_list(&[], output);
    }
    let backend = conn.get_database_backend();

    let mut sql = String::from(
        "SELECT checkpoint_id, session_id, scope, parent_commit, tree_oid, \
                metadata_blob_oid, traces_commit, created_at \
         FROM agent_checkpoint WHERE 1=1",
    );
    let mut values: Vec<sea_orm::Value> = Vec::new();
    if let Some(session) = &args.session {
        sql.push_str(" AND session_id = ?");
        values.push(session.clone().into());
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT 500");

    let rows = conn
        .query_all(Statement::from_sql_and_values(backend, &sql, values))
        .await
        .map_err(|e| CliError::fatal(format!("failed to query agent_checkpoint: {e}")))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(CheckpointRow {
            checkpoint_id: row.try_get_by("checkpoint_id").unwrap_or_default(),
            session_id: row.try_get_by("session_id").unwrap_or_default(),
            scope: row.try_get_by("scope").unwrap_or_default(),
            parent_commit: row.try_get_by("parent_commit").ok().flatten(),
            tree_oid: row.try_get_by("tree_oid").unwrap_or_default(),
            metadata_blob_oid: row.try_get_by("metadata_blob_oid").unwrap_or_default(),
            traces_commit: row.try_get_by("traces_commit").unwrap_or_default(),
            created_at: row.try_get_by("created_at").unwrap_or_default(),
        });
    }
    emit_list(&out, output)
}

async fn show(args: CheckpointShowArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    if !table_exists(&conn, "agent_checkpoint").await? {
        return Err(CliError::fatal(format!(
            "no checkpoint matches '{}': agent_checkpoint table not yet present (run `libra init`?)",
            args.checkpoint_id
        )));
    }
    let backend = conn.get_database_backend();
    let row = conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT checkpoint_id, session_id, scope, parent_commit, tree_oid, \
                    metadata_blob_oid, traces_commit, created_at \
             FROM agent_checkpoint WHERE checkpoint_id = ? LIMIT 1",
            [args.checkpoint_id.clone().into()],
        ))
        .await
        .map_err(|e| CliError::fatal(format!("failed to query agent_checkpoint: {e}")))?;
    match row {
        Some(row) => {
            let payload = CheckpointRow {
                checkpoint_id: row.try_get_by("checkpoint_id").unwrap_or_default(),
                session_id: row.try_get_by("session_id").unwrap_or_default(),
                scope: row.try_get_by("scope").unwrap_or_default(),
                parent_commit: row.try_get_by("parent_commit").ok().flatten(),
                tree_oid: row.try_get_by("tree_oid").unwrap_or_default(),
                metadata_blob_oid: row.try_get_by("metadata_blob_oid").unwrap_or_default(),
                traces_commit: row.try_get_by("traces_commit").unwrap_or_default(),
                created_at: row.try_get_by("created_at").unwrap_or_default(),
            };
            // Best-effort metadata blob load: if the user is in a libra
            // workspace, read the metadata.json blob and surface it; if
            // path resolution fails (e.g. running from outside any libra
            // repo), fall back to the row-only render rather than erroring.
            let metadata = load_metadata_blob(&payload.metadata_blob_oid).ok();
            // Best-effort tree summary: walk the checkpoint commit's root
            // tree to surface its leaf blobs (metadata.json,
            // transcript/<provider>, optional events/<provider>.jsonl) plus
            // the redacted transcript's byte length, per entire.md §7.3
            // ("metadata + transcript 长度 + tree 摘要"). Swallow errors so
            // running outside a workspace still renders the row.
            let tree_summary = summarize_checkpoint_tree(&payload.tree_oid).ok();
            emit_one(&payload, metadata.as_deref(), tree_summary.as_ref(), output)
        }
        None => Err(CliError::fatal(format!(
            "no checkpoint matches id '{}'",
            args.checkpoint_id
        ))),
    }
}

/// `libra agent checkpoint rewind <id> [--dry-run|--apply]`.
///
/// `dry-run` (the default when neither flag is set) lists the files the
/// checkpoint's `parent_commit` snapshot would restore, without touching the
/// worktree. `--apply` actually runs the worktree restore (delegating to the
/// existing `restore --source <parent_commit>` path), truncates supported
/// agent transcripts when possible, and leaves HEAD plus `refs/heads/*`
/// untouched per `docs/improvement/entire.md` §7.3.
async fn rewind(args: CheckpointRewindArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    if !table_exists(&conn, "agent_checkpoint").await? {
        return Err(CliError::fatal(format!(
            "no checkpoint matches '{}': agent_checkpoint table not yet present (run `libra init`?)",
            args.checkpoint_id
        )));
    }
    let backend = conn.get_database_backend();
    let row = conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT parent_commit, traces_commit FROM agent_checkpoint \
             WHERE checkpoint_id = ? LIMIT 1",
            [args.checkpoint_id.clone().into()],
        ))
        .await
        .map_err(|e| CliError::fatal(format!("failed to query agent_checkpoint: {e}")))?
        .ok_or_else(|| {
            CliError::fatal(format!("no checkpoint matches id '{}'", args.checkpoint_id))
        })?;

    let parent_commit: Option<String> = row.try_get_by("parent_commit").ok().flatten();
    let traces_commit: String = row.try_get_by("traces_commit").unwrap_or_default();

    // Without a parent_commit (unborn HEAD at ingest time) there is nothing
    // to restore the worktree to. Surface a clear diagnostic rather than a
    // silent no-op.
    let parent_commit = match parent_commit {
        Some(c) if !c.is_empty() => c,
        _ => {
            return Err(CliError::fatal(format!(
                "checkpoint '{}' has no recorded parent_commit (unborn HEAD or pre-commit ingest); \
                 nothing to rewind to. checkpoint commit: {traces_commit}",
                args.checkpoint_id
            )));
        }
    };

    // Resolve the parent commit's tree and enumerate files that would be
    // restored. We use this both for dry-run output and for a "summary
    // before apply" line.
    let parent_oid = ObjectHash::from_str(&parent_commit).map_err(|e| {
        CliError::fatal(format!(
            "checkpoint '{}' has invalid parent_commit '{parent_commit}': {e}",
            args.checkpoint_id
        ))
    })?;
    // Codex Phase-2-followups round-1 P1 #2: dry-run was previously
    // emitting only the additions/modifications side, leaving users
    // surprised when `--apply` also DELETED tracked files that were absent
    // from the target commit. The plan now surfaces both sides:
    //   restore = files in the target commit's tree (will be written)
    //   delete  = files tracked by the index but absent from the target
    //             tree (will be removed by the worktree-restore pass)
    let plan = build_rewind_plan(&parent_oid).map_err(|e| {
        CliError::fatal(format!("failed to enumerate files for rewind preview: {e}"))
    })?;

    // Codex round-2 follow-up: report `transcript_truncation_supported`
    // based on the actual `agent_kind` for this checkpoint, not a flat
    // `true`. Only `claude_code` has a TranscriptTruncator adapter today;
    // other kinds dispatch to `SkippedUnsupportedKind` at apply time, so
    // dry-run should mirror that.
    let truncation_supported = lookup_truncation_support(&conn, &args.checkpoint_id)
        .await
        .unwrap_or(false);

    if !args.apply {
        // dry-run path. We arrived here because either `--dry-run` was
        // explicit or neither flag was passed.
        if output.is_json() {
            let payload = serde_json::json!({
                "checkpoint_id": args.checkpoint_id,
                "parent_commit": parent_commit,
                "traces_commit": traces_commit,
                "would_restore_paths": plan.restore,
                "would_delete_paths": plan.delete,
                "applied": false,
                "transcript_truncation_supported": truncation_supported,
            });
            return emit_json_data("agent_checkpoint_rewind", &payload, output);
        }
        if output.quiet {
            return Ok(());
        }
        println!("Dry run — no files modified.");
        println!("checkpoint_id : {}", args.checkpoint_id);
        println!("parent_commit : {parent_commit}");
        println!("traces_commit : {traces_commit}");
        println!("would restore {} path(s):", plan.restore.len());
        for path in &plan.restore {
            println!("  + {path}");
        }
        println!("would delete  {} path(s):", plan.delete.len());
        for path in &plan.delete {
            println!("  - {path}");
        }
        println!(
            "Re-run with --apply to restore the working tree. For Claude \
             Code sessions the agent's transcript will be truncated to \
             the checkpoint boundary; other agent kinds keep the transcript \
             untouched."
        );
        return Ok(());
    }

    // --apply path: drive the typed restore for working-tree only,
    // matching the dry-run preview's file set. Re-using `restore` keeps
    // the LFS / index / pathspec semantics consistent with the rest of
    // the CLI.
    use crate::command::restore::{RestoreArgs, execute_checked_typed};
    let restore_args = RestoreArgs {
        pathspec: vec![".".to_string()],
        source: Some(parent_commit.clone()),
        worktree: true,
        staged: false,
    };
    execute_checked_typed(restore_args)
        .await
        .map_err(|e| CliError::fatal(format!("rewind --apply failed: {e}")))?;

    // Phase 4.1 (entire.md §14.4 item 1): if the captured agent has a
    // `TranscriptTruncator` adapter, call it to drop transcript lines
    // whose timestamp is strictly after the checkpoint boundary. This
    // closes the v1 caveat that the agent's local transcript was left
    // dangling after a worktree rewind.
    let truncation_outcome = truncate_agent_transcript_for_checkpoint(&args.checkpoint_id).await;

    if output.is_json() {
        let payload = serde_json::json!({
            "checkpoint_id": args.checkpoint_id,
            "parent_commit": parent_commit,
            "traces_commit": traces_commit,
            "restored_paths": plan.restore,
            "deleted_paths": plan.delete,
            "applied": true,
            "transcript_truncation": truncation_outcome.as_json(),
        });
        return emit_json_data("agent_checkpoint_rewind", &payload, output);
    }
    if !output.quiet {
        println!(
            "Restored {} path(s), deleted {} path(s) from {parent_commit}.",
            plan.restore.len(),
            plan.delete.len()
        );
        match &truncation_outcome {
            TranscriptTruncationOutcome::Truncated {
                path,
                lines_dropped,
            } => {
                println!(
                    "Truncated transcript {}: {} line(s) past the checkpoint dropped.",
                    path, lines_dropped
                );
            }
            TranscriptTruncationOutcome::NoChange { path } => {
                println!(
                    "Transcript {} already aligned with the checkpoint — no changes.",
                    path
                );
            }
            TranscriptTruncationOutcome::SkippedNoPath => {
                println!(
                    "Note: agent_session.metadata_json has no transcript_path; \
                     the agent's local transcript was left untouched."
                );
            }
            TranscriptTruncationOutcome::SkippedUnsupportedKind { agent_kind } => {
                println!(
                    "Note: agent_kind '{}' has no TranscriptTruncator adapter yet; \
                     the agent's local transcript was left untouched.",
                    agent_kind
                );
            }
            TranscriptTruncationOutcome::Failed { reason } => {
                eprintln!(
                    "warning: transcript truncation failed: {reason}. \
                     The worktree restore succeeded; the agent's transcript file \
                     was left as-is."
                );
            }
        }
    }
    Ok(())
}

/// Outcome of attempting transcript truncation alongside `rewind --apply`.
/// We never propagate these as hard errors — the worktree restore is the
/// load-bearing operation; transcript truncation is informational and a
/// failure here should not roll back the user's tree.
enum TranscriptTruncationOutcome {
    Truncated { path: String, lines_dropped: usize },
    NoChange { path: String },
    SkippedNoPath,
    SkippedUnsupportedKind { agent_kind: String },
    Failed { reason: String },
}

impl TranscriptTruncationOutcome {
    fn as_json(&self) -> serde_json::Value {
        // Codex round-4 follow-up: align `supported` semantics across
        // dry-run and apply outputs. `supported` here means "did the
        // truncator actually run end-to-end on this checkpoint?" — same
        // contract as `lookup_truncation_support` in the dry-run path.
        // Skipped paths therefore report `supported: false`; only
        // Truncated/NoChange (which exercised the adapter) and Failed
        // (which started the adapter) report `supported: true`.
        match self {
            Self::Truncated {
                path,
                lines_dropped,
            } => serde_json::json!({
                "supported": true,
                "applied": true,
                "transcript_path": path,
                "lines_dropped": lines_dropped,
            }),
            Self::NoChange { path } => serde_json::json!({
                "supported": true,
                "applied": false,
                "transcript_path": path,
                "reason": "transcript already aligned with checkpoint boundary",
            }),
            Self::SkippedNoPath => serde_json::json!({
                "supported": false,
                "applied": false,
                "reason": "agent_session.metadata_json has no transcript_path",
            }),
            Self::SkippedUnsupportedKind { agent_kind } => serde_json::json!({
                "supported": false,
                "applied": false,
                "agent_kind": agent_kind,
                "reason": "no TranscriptTruncator adapter for this agent_kind",
            }),
            Self::Failed { reason } => serde_json::json!({
                // Adapter was selected and started running but failed
                // mid-stream (e.g. concurrent writer, bad created_at).
                // Adapter IS supported; the apply just did not
                // succeed.
                "supported": true,
                "applied": false,
                "error": reason,
            }),
        }
    }
}

/// Look up the `agent_session` row paired with `checkpoint_id`, decide
/// whether we have an adapter for its `agent_kind`, then invoke the
/// truncator with a boundary derived from `agent_checkpoint.created_at`.
/// Returns the outcome rather than an error so the caller can surface a
/// uniform message no matter the path taken.
async fn truncate_agent_transcript_for_checkpoint(
    checkpoint_id: &str,
) -> TranscriptTruncationOutcome {
    let conn = get_db_conn_instance().await;
    truncate_agent_transcript_for_checkpoint_with_conn(&conn, checkpoint_id).await
}

/// Cheap "will `--apply` actually run a TranscriptTruncator for this
/// checkpoint?" probe used by the dry-run path so its
/// `transcript_truncation_supported` flag matches what `--apply` will
/// actually do.
///
/// Codex round-3 follow-up: this now considers BOTH conditions — the
/// adapter registry exposes a `TranscriptTruncator` for `agent_kind`
/// AND `metadata_json` has a non-empty `transcript_path`. Previously a
/// supported session whose `metadata_json` lacked `transcript_path`
/// would report `supported: true` but apply would short-circuit to
/// `SkippedNoPath`, contradicting the dry-run preview.
async fn lookup_truncation_support(
    conn: &sea_orm::DatabaseConnection,
    checkpoint_id: &str,
) -> Result<bool, sea_orm::DbErr> {
    let backend = conn.get_database_backend();
    let row = conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT s.agent_kind AS agent_kind, \
                    COALESCE(s.metadata_json, '{}') AS metadata_json \
             FROM agent_checkpoint cp \
             JOIN agent_session s ON s.session_id = cp.session_id \
             WHERE cp.checkpoint_id = ? LIMIT 1",
            [checkpoint_id.into()],
        ))
        .await?;
    let Some(r) = row else {
        return Ok(false);
    };
    let kind: String = r.try_get_by("agent_kind").unwrap_or_default();
    let metadata_json: String = r.try_get_by("metadata_json").unwrap_or_default();
    // Dispatch the truncator-support probe through the v0.17.677
    // capability registry instead of a literal "claude_code" match.
    // Mirrors the dispatch path in
    // `truncate_agent_transcript_for_checkpoint_with_conn` — both
    // sites must answer "would the truncator fire?" the same way so
    // the dry-run preview matches what `--apply` actually does.
    use crate::internal::ai::observed_agents::{AgentKind, truncator_for};
    let truncator_available = AgentKind::from_db_str(&kind)
        .and_then(truncator_for)
        .is_some();
    if !truncator_available {
        return Ok(false);
    }
    let has_transcript_path = serde_json::from_str::<serde_json::Value>(&metadata_json)
        .ok()
        .and_then(|v| {
            v.get("transcript_path")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .is_some_and(|s| !s.is_empty());
    Ok(has_transcript_path)
}

/// Connection-bound core of [`truncate_agent_transcript_for_checkpoint`].
/// Extracted so fixture tests can run against an in-memory SQLite without
/// the process-wide `get_db_conn_instance` singleton.
async fn truncate_agent_transcript_for_checkpoint_with_conn(
    conn: &sea_orm::DatabaseConnection,
    checkpoint_id: &str,
) -> TranscriptTruncationOutcome {
    use crate::internal::ai::observed_agents::{
        rfc3339_boundary_for_unix_seconds, write_truncated_transcript,
    };

    let backend = conn.get_database_backend();

    // Pull the session join for this checkpoint. We need:
    //  - agent_kind (to dispatch),
    //  - metadata_json (to find transcript_path) — coalesced to '{}'
    //    so legacy rows with NULL values don't error the SELECT
    //    (Codex round-1 P4 follow-up),
    //  - created_at on the checkpoint (the boundary).
    let row = match conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT s.agent_kind AS agent_kind, \
                    COALESCE(s.metadata_json, '{}') AS metadata_json, \
                    cp.created_at AS created_at \
             FROM agent_checkpoint cp \
             JOIN agent_session s ON s.session_id = cp.session_id \
             WHERE cp.checkpoint_id = ? LIMIT 1",
            [checkpoint_id.into()],
        ))
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return TranscriptTruncationOutcome::Failed {
                reason: format!(
                    "no agent_session join for checkpoint '{checkpoint_id}' \
                     (catalog row missing or schema mismatch)"
                ),
            };
        }
        Err(err) => {
            return TranscriptTruncationOutcome::Failed {
                reason: format!("agent_session lookup failed: {err}"),
            };
        }
    };
    let agent_kind: String = row.try_get_by("agent_kind").unwrap_or_default();
    let metadata_json: String = row.try_get_by("metadata_json").unwrap_or_default();
    let created_at: i64 = row.try_get_by("created_at").unwrap_or(0);

    let transcript_path: Option<String> = serde_json::from_str::<serde_json::Value>(&metadata_json)
        .ok()
        .and_then(|v| {
            v.get("transcript_path")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    let Some(path_str) = transcript_path else {
        return TranscriptTruncationOutcome::SkippedNoPath;
    };
    let path = std::path::PathBuf::from(&path_str);

    // Dispatch the truncator through the v0.17.677 capability registry
    // instead of a hard-coded `kind == "claude_code"` literal. The
    // registry handles three failure shapes:
    //   * `AgentKind::from_db_str` fails for unknown tags (schema
    //     mismatch — unsupported kind for this row).
    //   * `truncator_for` returns `None` for kinds whose adapter
    //     doesn't implement `TranscriptTruncator` (Factory AI Droid
    //     today). Adding another truncator implementation is a
    //     single-arm change in `observed_agents::mod.rs::truncator_for`
    //     and the new kind is dispatched here automatically.
    use crate::internal::ai::observed_agents::{AgentKind, truncator_for};
    let Some(parsed_kind) = AgentKind::from_db_str(&agent_kind) else {
        return TranscriptTruncationOutcome::SkippedUnsupportedKind { agent_kind };
    };
    let Some(agent) = truncator_for(parsed_kind) else {
        return TranscriptTruncationOutcome::SkippedUnsupportedKind { agent_kind };
    };
    // Capture the file size at read time so `write_truncated_transcript`
    // (and the NoChange early-return below) can detect a concurrent
    // writer that grew the file before our rename. Codex round-1 P2 +
    // round-2 follow-up.
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return TranscriptTruncationOutcome::Failed {
                reason: format!("transcript file '{path_str}' not found"),
            };
        }
        Err(err) => {
            return TranscriptTruncationOutcome::Failed {
                reason: format!("read transcript '{path_str}': {err}"),
            };
        }
    };
    let size_at_read = bytes.len() as u64;
    // Codex round-2 follow-up: invalid `created_at` propagates as a
    // `Failed` outcome rather than silently degrading to the Unix epoch
    // (which would erase the whole transcript next time around).
    let boundary = match rfc3339_boundary_for_unix_seconds(created_at) {
        Ok(b) => b,
        Err(err) => {
            return TranscriptTruncationOutcome::Failed {
                reason: format!("rfc3339_boundary_for_unix_seconds: {err}"),
            };
        }
    };
    let truncated = match agent.truncate_transcript(&bytes, &boundary) {
        Ok(t) => t,
        Err(err) => {
            return TranscriptTruncationOutcome::Failed {
                reason: format!("truncate_transcript: {err}"),
            };
        }
    };
    if truncated == bytes {
        // Codex round-2 follow-up: even on the no-change path, re-stat
        // the original to make sure no concurrent writer appended new
        // bytes between our read and now. If the file grew, we still
        // should not return "already aligned" — those new bytes might
        // be post-boundary and the user expects them dropped.
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() != size_at_read => {
                return TranscriptTruncationOutcome::Failed {
                    reason: format!(
                        "transcript '{path_str}' grew from {} to {} bytes during \
                         truncation (concurrent writer); rerun once the agent is idle",
                        size_at_read,
                        meta.len()
                    ),
                };
            }
            Ok(_) => {}
            Err(err) => {
                return TranscriptTruncationOutcome::Failed {
                    reason: format!("re-stat transcript '{path_str}': {err}"),
                };
            }
        }
        return TranscriptTruncationOutcome::NoChange { path: path_str };
    }
    let lines_before = bytes.iter().filter(|&&b| b == b'\n').count();
    let lines_after = truncated.iter().filter(|&&b| b == b'\n').count();
    let lines_dropped = lines_before.saturating_sub(lines_after);
    if let Err(err) = write_truncated_transcript(&path, &truncated, Some(size_at_read)) {
        return TranscriptTruncationOutcome::Failed {
            reason: format!("write_truncated_transcript: {err}"),
        };
    }
    TranscriptTruncationOutcome::Truncated {
        path: path_str,
        lines_dropped,
    }
}

/// Files affected by a `rewind --apply`, broken down by side. `restore`
/// = present in the target commit (will be written to the worktree
/// after `--apply`); `delete` = tracked by the index but absent from the
/// target commit's tree (will be removed from the worktree by the
/// underlying restore's deleted-files pass — see
/// `command::restore::restore_worktree_tracked`).
struct RewindPlan {
    restore: Vec<String>,
    delete: Vec<String>,
}

fn build_rewind_plan(commit_oid: &ObjectHash) -> Result<RewindPlan, anyhow::Error> {
    use std::{collections::HashSet, path::PathBuf};

    use git_internal::internal::index::Index;

    let commit: Commit = load_object(commit_oid)
        .map_err(|e| anyhow::anyhow!("failed to load commit {commit_oid}: {e}"))?;
    let tree: Tree = load_object(&commit.tree_id)
        .map_err(|e| anyhow::anyhow!("failed to load tree {}: {e}", commit.tree_id))?;
    let target: Vec<(PathBuf, ObjectHash)> = tree.get_plain_items();
    let target_set: HashSet<PathBuf> = target.iter().map(|(p, _)| p.clone()).collect();

    let mut restore: Vec<String> = target
        .iter()
        .map(|(p, _)| p.display().to_string())
        .collect();
    restore.sort();

    // The index is the authoritative tracked-files view. Any path tracked
    // there but absent from the target tree will be removed by the
    // worktree restore — surface it in the dry-run so users see both
    // sides of the diff.
    let mut delete: Vec<String> = match Index::load(crate::utils::path::index()) {
        Ok(index) => index
            .tracked_entries(0)
            .into_iter()
            .filter_map(|entry| {
                let path = PathBuf::from(&entry.name);
                if target_set.contains(&path) {
                    None
                } else {
                    Some(path.display().to_string())
                }
            })
            .collect(),
        // Index unreadable (e.g. fresh repo with no staged files) — leave
        // the deletion set empty and let the user proceed with --apply.
        Err(_) => Vec::new(),
    };
    delete.sort();

    Ok(RewindPlan { restore, delete })
}

/// One leaf blob in a checkpoint commit's tree.
#[derive(Debug, Serialize)]
struct CheckpointTreeEntry {
    /// Full path within the checkpoint commit tree, e.g.
    /// `checkpoint/<ab>/<rest>/transcript/claude_code`.
    path: String,
    /// Blob size in bytes.
    size: usize,
}

/// Summary of a checkpoint commit's tree contents — the per-checkpoint
/// `metadata.json`, the redacted `transcript/<provider>` blob, and any
/// `events/<provider>.jsonl` — surfaced by `checkpoint show` per
/// entire.md §7.3 ("metadata + transcript 长度 + tree 摘要").
#[derive(Debug, Serialize)]
struct CheckpointTreeSummary {
    entries: Vec<CheckpointTreeEntry>,
    /// Byte length of the redacted transcript blob (the entry whose parent
    /// directory is `transcript`), if the tree carries one.
    transcript_bytes: Option<usize>,
}

/// Walk a checkpoint commit's root tree (`tree_oid`) and summarise the leaf
/// blobs it carries plus the redacted transcript's byte length.
///
/// Best-effort: returns `Err` when not inside a libra workspace or when the
/// objects cannot be read, so `checkpoint show` falls back to a row-only
/// render rather than failing. The transcript blob lives at
/// `checkpoint/<ab>/<rest>/transcript/<provider>` (see
/// `HistoryManager::append_checkpoint_commit`), so it is identified by a
/// parent directory named `transcript`.
fn summarize_checkpoint_tree(tree_oid: &str) -> Result<CheckpointTreeSummary, anyhow::Error> {
    use std::path::Path;

    let oid = ObjectHash::from_str(tree_oid)
        .map_err(|e| anyhow::anyhow!("invalid tree_oid '{tree_oid}': {e}"))?;
    let tree: Tree = load_object(&oid)
        .map_err(|e| anyhow::anyhow!("failed to load checkpoint tree {tree_oid}: {e}"))?;
    let storage = util::try_get_storage_path(None)
        .map_err(|e| anyhow::anyhow!("not in a libra repository: {e}"))?;

    let items = tree.get_plain_items();
    let mut entries = Vec::with_capacity(items.len());
    let mut transcript_bytes = None;
    for (path, hash) in &items {
        let size = read_git_object(&storage, hash)
            .map_err(|e| anyhow::anyhow!("failed to read blob {hash}: {e}"))?
            .len();
        if path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "transcript")
        {
            transcript_bytes = Some(size);
        }
        entries.push(CheckpointTreeEntry {
            path: path.display().to_string(),
            size,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(CheckpointTreeSummary {
        entries,
        transcript_bytes,
    })
}

fn load_metadata_blob(oid: &str) -> Result<String, CliError> {
    let hash = ObjectHash::from_str(oid)
        .map_err(|e| CliError::fatal(format!("invalid metadata_blob_oid '{oid}': {e}")))?;
    let storage = util::try_get_storage_path(None)
        .map_err(|e| CliError::fatal(format!("not in a libra repository: {e}")))?;
    let raw = read_git_object(&storage, &hash).map_err(|e| {
        CliError::fatal(format!(
            "failed to read metadata blob {oid} from object store: {e}"
        ))
    })?;
    String::from_utf8(raw)
        .map_err(|e| CliError::fatal(format!("metadata blob {oid} is not UTF-8: {e}")))
}

fn emit_list(rows: &[CheckpointRow], output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("agent_checkpoints", &rows, output);
    }
    if output.quiet {
        return Ok(());
    }
    if rows.is_empty() {
        println!("(no captured checkpoints)");
        return Ok(());
    }
    println!(
        "{:<37}  {:<37}  {:<10}  {:<20}",
        "checkpoint_id", "session_id", "scope", "created_at"
    );
    for r in rows {
        println!(
            "{:<37}  {:<37}  {:<10}  {:<20}",
            r.checkpoint_id, r.session_id, r.scope, r.created_at
        );
    }
    Ok(())
}

fn emit_one(
    row: &CheckpointRow,
    metadata_blob: Option<&str>,
    tree: Option<&CheckpointTreeSummary>,
    output: &OutputConfig,
) -> CliResult<()> {
    if output.is_json() {
        // Inline the metadata content as parsed JSON so JSON consumers can
        // join on it without doing a second blob fetch.
        let metadata_json = metadata_blob
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .unwrap_or(serde_json::Value::Null);
        let payload = serde_json::json!({
            "checkpoint": row,
            "metadata": metadata_json,
            "tree": tree,
            "transcript_bytes": tree.and_then(|t| t.transcript_bytes),
        });
        return emit_json_data("agent_checkpoint", &payload, output);
    }
    if output.quiet {
        return Ok(());
    }
    println!("checkpoint_id     : {}", row.checkpoint_id);
    println!("session_id        : {}", row.session_id);
    println!("scope             : {}", row.scope);
    let parent_display = match row.parent_commit.as_deref() {
        Some(commit) if !commit.is_empty() => commit,
        _ => "(none — unborn HEAD or pre-commit ingest)",
    };
    println!("parent_commit     : {parent_display}");
    println!("tree_oid          : {}", row.tree_oid);
    println!("metadata_blob_oid : {}", row.metadata_blob_oid);
    println!("traces_commit     : {}", row.traces_commit);
    println!("created_at        : {}", row.created_at);
    if let Some(tree) = tree {
        match tree.transcript_bytes {
            Some(n) => println!("transcript_bytes  : {n}"),
            None => println!("transcript_bytes  : (no transcript entry in tree)"),
        }
        println!("tree summary ({} entr{}):", tree.entries.len(), {
            if tree.entries.len() == 1 { "y" } else { "ies" }
        });
        for e in &tree.entries {
            println!("  {:>10}  {}", e.size, e.path);
        }
    }
    if let Some(metadata) = metadata_blob {
        println!("---");
        println!("metadata.json:");
        println!("{metadata}");
    }
    Ok(())
}

async fn table_exists(conn: &(impl ConnectionTrait + ?Sized), name: &str) -> CliResult<bool> {
    let backend = conn.get_database_backend();
    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1",
        [name.into()],
    );
    conn.query_one(stmt)
        .await
        .map(|row| row.is_some())
        .map_err(|e| CliError::fatal(format!("failed to query sqlite_master: {e}")))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sea_orm::{ConnectOptions, Database, ExecResult};
    use tempfile::TempDir;

    use super::*;
    use crate::internal::db::{
        ensure_ai_runtime_contract_schema, migration::run_builtin_migrations,
    };

    const LEGACY_BOOTSTRAP_SQL: &str = include_str!("../../../sql/sqlite_20260309_init.sql");

    /// Spin up a freshly-migrated SQLite at `<dir>/libra.db`. Mirrors the
    /// fixture used by the hook runtime tests so the schema is identical
    /// to production (legacy bootstrap → AI runtime contract → registered
    /// migrations).
    async fn fresh_db() -> (TempDir, sea_orm::DatabaseConnection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("libra.db");
        std::fs::File::create(&path).unwrap();
        let url = format!("sqlite://{}", path.display());
        let mut opts = ConnectOptions::new(url);
        opts.sqlx_logging(false);
        let conn = Database::connect(opts).await.unwrap();
        let backend = conn.get_database_backend();
        for raw in LEGACY_BOOTSTRAP_SQL.split(';') {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let _: ExecResult = conn
                .execute(Statement::from_string(backend, trimmed.to_string()))
                .await
                .unwrap_or_else(|e| panic!("legacy bootstrap stmt failed: {trimmed}\n{e}"));
        }
        ensure_ai_runtime_contract_schema(&conn).await.unwrap();
        run_builtin_migrations(&conn).await.unwrap();
        (dir, conn)
    }

    /// Phase 4.1 acceptance: when the fixture has a Claude Code session
    /// with a `transcript_path` in `metadata_json` and a checkpoint
    /// timestamped between two transcript lines, the truncator must
    /// drop the post-boundary lines.
    #[tokio::test]
    async fn rewind_truncate_drops_post_boundary_lines_for_claude_code() {
        let (dir, conn) = fresh_db().await;
        // Create the on-disk transcript with two lines straddling the
        // boundary. The checkpoint lives at 10:30; the second line at
        // 11:00 must be dropped.
        let transcript_path = dir.path().join("session.jsonl");
        fs::write(
            &transcript_path,
            b"{\"timestamp\":\"2026-05-05T10:00:00Z\",\"text\":\"keep\"}\n\
              {\"timestamp\":\"2026-05-05T11:00:00Z\",\"text\":\"drop\"}\n",
        )
        .unwrap();
        let metadata_json = serde_json::json!({
            "transcript_path": transcript_path.to_str().unwrap(),
        })
        .to_string();
        // Boundary at 2026-05-05T10:30:00Z so the 10:00 line is kept and
        // the 11:00 line is dropped.
        let created_at: i64 = chrono::DateTime::parse_from_rfc3339("2026-05-05T10:30:00Z")
            .unwrap()
            .timestamp();

        let backend = conn.get_database_backend();
        conn.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO agent_session (
                session_id, agent_kind, provider_session_id, state, working_dir,
                metadata_json, redaction_report, started_at, last_event_at
             ) VALUES ('s-1', 'claude_code', 'p-1', 'stopped', '/tmp', ?, '{}', 0, 0)",
            [metadata_json.into()],
        ))
        .await
        .unwrap();
        conn.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO agent_checkpoint (
                checkpoint_id, session_id, scope, parent_commit, tree_oid,
                metadata_blob_oid, traces_commit, created_at
             ) VALUES ('cp-1', 's-1', 'committed', NULL, 'tree', 'meta', 'commit', ?)",
            [created_at.into()],
        ))
        .await
        .unwrap();

        let outcome =
            super::truncate_agent_transcript_for_checkpoint_with_conn(&conn, "cp-1").await;
        match outcome {
            super::TranscriptTruncationOutcome::Truncated { lines_dropped, .. } => {
                assert_eq!(lines_dropped, 1, "exactly one line removed");
            }
            other => panic!("expected Truncated, got {:?}", other.as_json()),
        }

        let after = fs::read_to_string(&transcript_path).unwrap();
        assert!(after.contains("\"keep\""));
        assert!(!after.contains("\"drop\""));
    }

    /// When `metadata_json` lacks a transcript_path, the helper must
    /// surface `SkippedNoPath` rather than failing.
    #[tokio::test]
    async fn rewind_truncate_skips_when_no_transcript_path_in_metadata() {
        let (_dir, conn) = fresh_db().await;
        let backend = conn.get_database_backend();
        conn.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO agent_session (
                session_id, agent_kind, provider_session_id, state, working_dir,
                metadata_json, redaction_report, started_at, last_event_at
             ) VALUES ('s-2', 'claude_code', 'p-2', 'stopped', '/tmp', '{}', '{}', 0, 0)",
            [],
        ))
        .await
        .unwrap();
        conn.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO agent_checkpoint (
                checkpoint_id, session_id, scope, parent_commit, tree_oid,
                metadata_blob_oid, traces_commit, created_at
             ) VALUES ('cp-2', 's-2', 'committed', NULL, 't', 'm', 'c', 0)",
            [],
        ))
        .await
        .unwrap();

        let outcome =
            super::truncate_agent_transcript_for_checkpoint_with_conn(&conn, "cp-2").await;
        assert!(matches!(
            outcome,
            super::TranscriptTruncationOutcome::SkippedNoPath
        ));
    }

    /// Codex round-3 follow-up: the dry-run `transcript_truncation_supported`
    /// flag must match the apply path's actual decision. We test all four
    /// quadrants (kind × has_transcript_path) against
    /// `lookup_truncation_support`.
    #[tokio::test]
    async fn lookup_truncation_support_matches_apply_decision() {
        let (dir, conn) = fresh_db().await;
        let backend = conn.get_database_backend();

        let transcript_path = dir.path().join("session.jsonl");
        fs::write(&transcript_path, b"").unwrap();
        let path_meta = serde_json::json!({
            "transcript_path": transcript_path.to_str().unwrap(),
        })
        .to_string();

        for (idx, (kind, meta, expected)) in [
            ("claude_code", path_meta.as_str(), true),
            ("gemini", path_meta.as_str(), true),
            ("cursor", path_meta.as_str(), true),
            ("cursor", "{}", false),
            ("factory_ai", path_meta.as_str(), false),
        ]
        .iter()
        .enumerate()
        {
            let session_id = format!("s-{idx}");
            let provider_session_id = format!("p-{idx}");
            let checkpoint_id = format!("cp-{idx}");
            conn.execute(Statement::from_sql_and_values(
                backend,
                "INSERT INTO agent_session (
                    session_id, agent_kind, provider_session_id, state, working_dir,
                    metadata_json, redaction_report, started_at, last_event_at
                 ) VALUES (?, ?, ?, 'stopped', '/tmp', ?, '{}', 0, 0)",
                [
                    session_id.clone().into(),
                    (*kind).into(),
                    provider_session_id.into(),
                    (*meta).into(),
                ],
            ))
            .await
            .unwrap();
            conn.execute(Statement::from_sql_and_values(
                backend,
                "INSERT INTO agent_checkpoint (
                    checkpoint_id, session_id, scope, parent_commit, tree_oid,
                    metadata_blob_oid, traces_commit, created_at
                 ) VALUES (?, ?, 'committed', NULL, 't', 'm', 'c', 0)",
                [checkpoint_id.clone().into(), session_id.into()],
            ))
            .await
            .unwrap();

            let supported = super::lookup_truncation_support(&conn, &checkpoint_id)
                .await
                .unwrap();
            assert_eq!(
                supported, *expected,
                "case {idx} (kind={kind}, meta={meta}) supported={supported}, expected={expected}"
            );
        }
    }

    /// When `agent_kind` has no truncator, the helper must report
    /// `SkippedUnsupportedKind` so the operator knows the transcript
    /// was deliberately not touched.
    #[tokio::test]
    async fn rewind_truncate_skips_unsupported_agent_kind() {
        let (dir, conn) = fresh_db().await;
        let transcript_path = dir.path().join("session.jsonl");
        fs::write(&transcript_path, b"{}\n").unwrap();
        let metadata_json = serde_json::json!({
            "transcript_path": transcript_path.to_str().unwrap(),
        })
        .to_string();

        let backend = conn.get_database_backend();
        conn.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO agent_session (
                session_id, agent_kind, provider_session_id, state, working_dir,
                metadata_json, redaction_report, started_at, last_event_at
             ) VALUES ('s-3', 'factory_ai', 'p-3', 'stopped', '/tmp', ?, '{}', 0, 0)",
            [metadata_json.into()],
        ))
        .await
        .unwrap();
        conn.execute(Statement::from_sql_and_values(
            backend,
            "INSERT INTO agent_checkpoint (
                checkpoint_id, session_id, scope, parent_commit, tree_oid,
                metadata_blob_oid, traces_commit, created_at
             ) VALUES ('cp-3', 's-3', 'committed', NULL, 't', 'm', 'c', 0)",
            [],
        ))
        .await
        .unwrap();

        let outcome =
            super::truncate_agent_transcript_for_checkpoint_with_conn(&conn, "cp-3").await;
        match outcome {
            super::TranscriptTruncationOutcome::SkippedUnsupportedKind { agent_kind } => {
                assert_eq!(agent_kind, "factory_ai");
            }
            other => panic!("expected SkippedUnsupportedKind, got {:?}", other.as_json()),
        }
    }
}
