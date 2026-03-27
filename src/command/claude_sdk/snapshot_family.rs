//! Codex-compatible snapshot projections for Claude formal bridge objects.
//!
//! These objects remain read-model projections in Libra history. Formal lifecycle
//! semantics continue to live in git-internal events such as `intent_event`,
//! `run_event`, and `plan_step_event`.

use git_internal::{
    hash::ObjectHash,
    internal::object::{
        intent::Intent, patchset::PatchSet, plan::Plan, provenance::Provenance, run::Run,
    },
};

use super::*;
use crate::{
    internal::ai::codex::{
        model::{
            IntentSnapshot, PatchSetSnapshot, PlanSnapshot, PlanStepSnapshot, ProvenanceSnapshot,
            RunSnapshot, TaskSnapshot,
        },
        types::{FileChange, PatchStatus},
    },
    utils::storage::Storage,
};

pub(super) async fn ensure_full_family_intent_created(
    storage_path: &Path,
    ai_session_id: Option<&str>,
    intent_id: &str,
) -> Result<()> {
    let Some(ai_session_id) = ai_session_id else {
        return Ok(());
    };
    let intent: Intent =
        read_tracked_object(storage_path, "intent", intent_id, "formal intent").await?;
    let intent_snapshot = IntentSnapshot {
        id: intent_id.to_string(),
        content: intent.prompt().to_string(),
        thread_id: ai_session_id.to_string(),
        parents: intent.parents().iter().map(ToString::to_string).collect(),
        analysis_context_frames: intent
            .analysis_context_frames()
            .iter()
            .map(ToString::to_string)
            .collect(),
        created_at: intent.header().created_at(),
    };
    upsert_tracked_json_object(storage_path, "intent_snapshot", intent_id, &intent_snapshot)
        .await?;

    Ok(())
}

pub(super) async fn ensure_full_family_run_objects(
    storage_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
    audit_bundle: &ManagedAuditBundle,
) -> Result<()> {
    let run: Run =
        read_tracked_object(storage_path, "run", &run_binding.run_id, "formal run").await?;
    let run_snapshot = RunSnapshot {
        id: run_binding.run_id.clone(),
        thread_id: audit_bundle.bridge.object_candidates.thread_id.clone(),
        plan_id: run.plan().map(|id| id.to_string()),
        task_id: Some(run.task().to_string()),
        started_at: run.header().created_at(),
    };
    upsert_tracked_json_object(
        storage_path,
        "run_snapshot",
        &run_binding.run_id,
        &run_snapshot,
    )
    .await?;

    let provenance = match read_tracked_object(
        storage_path,
        "provenance",
        &format!("prov_{}", run_binding.run_id),
        "formal provenance",
    )
    .await
    {
        Ok(provenance) => provenance,
        Err(_) => find_run_provenance(storage_path, &run_binding.run_id).await?,
    };
    let provenance_snapshot_id = format!("prov_{}", run_binding.run_id);
    let provenance_snapshot = ProvenanceSnapshot {
        id: provenance_snapshot_id.clone(),
        run_id: run_binding.run_id.clone(),
        model: Some(provenance.model().to_string()),
        provider: Some(provenance.provider().to_string()),
        parameters: provenance
            .parameters()
            .cloned()
            .unwrap_or_else(|| json!({})),
        created_at: provenance.header().created_at(),
    };
    upsert_tracked_json_object(
        storage_path,
        "provenance_snapshot",
        &provenance_snapshot_id,
        &provenance_snapshot,
    )
    .await?;

    Ok(())
}

pub(super) async fn ensure_full_family_plan_objects(
    storage_path: &Path,
    ai_session_id: &str,
    run_binding: &ClaudeFormalRunBindingArtifact,
) -> Result<()> {
    let Some(plan_id) = run_binding.plan_id.as_deref() else {
        return Ok(());
    };
    let plan_uuid = plan_id
        .parse::<Uuid>()
        .map_err(|error| anyhow!("invalid formal plan id '{}': {error}", plan_id))?;
    let run_uuid = run_binding
        .run_id
        .parse::<Uuid>()
        .map_err(|error| anyhow!("invalid formal run id '{}': {error}", run_binding.run_id))?;
    let plan: Plan = read_tracked_object(storage_path, "plan", plan_id, "formal plan").await?;
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let actor = mcp_server
        .resolve_actor_from_params(Some("system"), Some("claude-sdk-snapshot"))
        .map_err(|error| anyhow!("failed to resolve Claude snapshot actor: {error:?}"))?;
    let plan_snapshot = PlanSnapshot {
        id: plan_id.to_string(),
        thread_id: ai_session_id.to_string(),
        intent_id: Some(plan.intent().to_string()),
        turn_id: Some(ai_session_id.to_string()),
        step_text: plan
            .steps()
            .iter()
            .map(|step| step.description().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        parents: plan.parents().iter().map(ToString::to_string).collect(),
        context_frames: plan
            .context_frames()
            .iter()
            .map(ToString::to_string)
            .collect(),
        created_at: plan.header().created_at(),
    };
    upsert_tracked_json_object(storage_path, "plan_snapshot", plan_id, &plan_snapshot).await?;

    for (ordinal, step) in plan.steps().iter().enumerate() {
        let step_uuid = stable_uuid_from_seed(&format!("claude-plan-step:{plan_id}:{ordinal}"))?;
        let step_id = step_uuid.to_string();
        let step_snapshot = PlanStepSnapshot {
            id: step_id.clone(),
            plan_id: plan_id.to_string(),
            text: step.description().to_string(),
            ordinal: ordinal as i64,
            created_at: plan.header().created_at(),
        };
        upsert_tracked_json_object(storage_path, "plan_step_snapshot", &step_id, &step_snapshot)
            .await?;

        // Keep per-step task_snapshot as a Codex-compatible projection only.
        // The Claude bridge does not materialize matching formal Task/TaskEvent
        // objects for each derived plan step; execution semantics live on
        // formal plan_step_event instead.
        let task_snapshot_id = format!("task_{}_{}", plan_id, ordinal);
        let task_snapshot = TaskSnapshot {
            id: task_snapshot_id.clone(),
            thread_id: ai_session_id.to_string(),
            plan_id: Some(plan_id.to_string()),
            intent_id: Some(plan.intent().to_string()),
            turn_id: Some(ai_session_id.to_string()),
            title: Some(step.description().to_string()),
            parent_task_id: None,
            origin_step_id: Some(step_id.clone()),
            dependencies: Vec::new(),
            created_at: plan.header().created_at(),
        };
        upsert_tracked_json_object(
            storage_path,
            "task_snapshot",
            &task_snapshot_id,
            &task_snapshot,
        )
        .await?;

        let _ = mcp_server
            .create_plan_step_event_impl(
                CreatePlanStepEventParams {
                    plan_id: plan_uuid.to_string(),
                    step_id: step_uuid.to_string(),
                    run_id: run_uuid.to_string(),
                    status: "pending".to_string(),
                    reason: Some("derived from formal Claude plan snapshot family".to_string()),
                    consumed_frames: None,
                    produced_frames: None,
                    spawned_task_id: None,
                    outputs: None,
                    actor_kind: Some("system".to_string()),
                    actor_id: Some("claude-sdk-snapshot".to_string()),
                },
                actor.clone(),
            )
            .await
            .map_err(|error| {
                anyhow!("failed to create derived formal plan_step_event: {error:?}")
            })?;
    }

    Ok(())
}

fn stable_uuid_from_seed(seed: &str) -> Result<Uuid> {
    let bytes = serde_json::to_vec(seed).context("failed to serialize UUID seed")?;
    let hash = ObjectHash::from_type_and_data(
        git_internal::internal::object::types::ObjectType::Blob,
        &bytes,
    );
    let hex = hash.to_string();
    let uuid_hex = &hex[..32];
    let formatted = format!(
        "{}-{}-{}-{}-{}",
        &uuid_hex[0..8],
        &uuid_hex[8..12],
        &uuid_hex[12..16],
        &uuid_hex[16..20],
        &uuid_hex[20..32]
    );
    Uuid::parse_str(&formatted)
        .with_context(|| format!("failed to build stable UUID from seed '{}'", seed))
}

pub(super) async fn ensure_full_family_patchset_snapshot(
    storage_path: &Path,
    run_binding: &ClaudeFormalRunBindingArtifact,
    patchset_id: &str,
) -> Result<()> {
    let patchset: PatchSet =
        read_tracked_object(storage_path, "patchset", patchset_id, "formal patchset").await?;
    let changes = build_patchset_snapshot_changes(storage_path, &patchset).await?;
    let snapshot = PatchSetSnapshot {
        id: patchset_id.to_string(),
        run_id: run_binding.run_id.clone(),
        thread_id: run_binding.ai_session_id.clone(),
        created_at: patchset.header().created_at(),
        status: PatchStatus::Completed,
        changes,
    };
    upsert_tracked_json_object(storage_path, "patchset_snapshot", patchset_id, &snapshot).await?;
    Ok(())
}

pub(super) async fn ensure_full_family_intent_completed(
    storage_path: &Path,
    _ai_session_id: &str,
    intent_id: Option<&str>,
) -> Result<()> {
    let Some(intent_id) = intent_id else {
        return Ok(());
    };
    let mcp_server = init_local_mcp_server(storage_path).await?;
    mcp_server
        .update_intent_impl(UpdateIntentParams {
            intent_id: intent_id.to_string(),
            status: Some("completed".to_string()),
            commit_sha: None,
            reason: Some("Claude terminal decision persisted".to_string()),
            next_intent_id: None,
        })
        .await
        .map_err(|error| anyhow!("failed to record formal intent completion event: {error:?}"))?;
    Ok(())
}

async fn find_run_provenance(storage_path: &Path, run_id: &str) -> Result<Provenance> {
    let mcp_server = init_local_mcp_server(storage_path).await?;
    let history = mcp_server
        .intent_history_manager
        .as_ref()
        .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
    let storage = LocalStorage::new(storage_path.join("objects"));
    for (object_id, object_hash) in history
        .list_objects("provenance")
        .await
        .context("failed to list provenance objects")?
    {
        let provenance = storage
            .get_json::<Provenance>(&object_hash)
            .await
            .with_context(|| format!("failed to read provenance '{object_id}'"))?;
        if provenance.run_id().to_string() == run_id {
            return Ok(provenance);
        }
    }
    bail!(
        "formal provenance for run '{}' does not exist in AI history",
        run_id
    )
}

async fn build_patchset_snapshot_changes(
    storage_path: &Path,
    patchset: &PatchSet,
) -> Result<Vec<FileChange>> {
    if let Some(artifact) = patchset.artifact() {
        let storage = LocalStorage::new(storage_path.join("objects"));
        let hash = artifact.key().parse::<ObjectHash>().map_err(|error| {
            anyhow!(
                "invalid patchset artifact hash '{}': {error}",
                artifact.key()
            )
        })?;
        let (bytes, _) = storage
            .get(&hash)
            .await
            .with_context(|| format!("failed to load patchset artifact '{}'", artifact.key()))?;
        let diff = String::from_utf8(bytes).context("patchset diff artifact is not valid UTF-8")?;
        return Ok(render_patchset_changes_from_diff(patchset, &diff));
    }

    Ok(patchset
        .touched()
        .iter()
        .map(|file| FileChange {
            path: file.path.clone(),
            diff: String::new(),
            change_type: format!("{:?}", file.change_type).to_lowercase(),
        })
        .collect())
}

fn render_patchset_changes_from_diff(patchset: &PatchSet, diff: &str) -> Vec<FileChange> {
    if patchset.touched().len() <= 1 {
        return patchset
            .touched()
            .iter()
            .map(|file| FileChange {
                path: file.path.clone(),
                diff: diff.to_string(),
                change_type: format!("{:?}", file.change_type).to_lowercase(),
            })
            .collect();
    }

    let mut sections = BTreeMap::<String, String>::new();
    let mut current_path = None::<String>;
    let mut current_lines = Vec::<String>::new();
    for line in diff.lines() {
        if let Some(path) = line
            .strip_prefix("diff --git a/")
            .and_then(|rest| rest.split(" b/").next())
            .map(ToString::to_string)
        {
            if let Some(path_key) = current_path.take() {
                sections.insert(path_key, current_lines.join("\n"));
            }
            current_path = Some(path);
            current_lines = vec![line.to_string()];
            continue;
        }
        if current_path.is_some() {
            current_lines.push(line.to_string());
        }
    }
    if let Some(path_key) = current_path {
        sections.insert(path_key, current_lines.join("\n"));
    }

    patchset
        .touched()
        .iter()
        .map(|file| FileChange {
            path: file.path.clone(),
            diff: sections.get(&file.path).cloned().unwrap_or_default(),
            change_type: format!("{:?}", file.change_type).to_lowercase(),
        })
        .collect()
}
