//! Codex-compatible snapshot projections for Claude formal bridge objects.
//!
//! These objects remain read-model projections in Libra history. Formal lifecycle
//! semantics continue to live in git-internal events such as `intent_event`,
//! `run_event`, and `plan_step_event`.

use std::collections::{HashMap, HashSet};

use git_internal::{
    hash::ObjectHash,
    internal::object::{
        intent::Intent,
        patchset::PatchSet,
        plan::Plan,
        plan_step_event::{PlanStepEvent, PlanStepStatus},
        provenance::Provenance,
        run::Run,
        task::Task,
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
    utils::{storage::Storage, storage_ext::StorageExt},
};

const DERIVED_PLAN_STEP_EVENT_REASON: &str = "derived from formal Claude plan snapshot family";
const DERIVED_PLAN_STEP_TASK_REASON: &str = "derived from formal Claude plan step";

type PlanStepEventKey = (Uuid, Uuid, Uuid);

struct DerivedPlanStepTaskSpec<'a> {
    root_task_id: Uuid,
    intent_id: Uuid,
    step_id: Uuid,
    title: &'a str,
    dependency_task_id: Option<Uuid>,
    plan_id: Uuid,
    run_id: Uuid,
}

struct PlanProjectionContext {
    mcp_server: Arc<LibraMcpServer>,
    task_ids_by_origin_step: HashMap<Uuid, Uuid>,
    latest_plan_step_statuses: HashMap<PlanStepEventKey, PlanStepStatus>,
    derived_plan_step_events: HashSet<PlanStepEventKey>,
}

impl PlanProjectionContext {
    async fn load(storage_path: &Path) -> Result<Self> {
        let mcp_server = init_local_mcp_server(storage_path).await?;
        let history = mcp_server
            .intent_history_manager
            .as_ref()
            .ok_or_else(|| anyhow!("local MCP history manager is unavailable"))?;
        let storage = LocalStorage::new(storage_path.join("objects"));
        let task_ids_by_origin_step =
            load_task_ids_by_origin_step(history.as_ref(), &storage).await?;
        let (latest_plan_step_statuses, derived_plan_step_events) =
            load_plan_step_event_indexes(history.as_ref(), &storage).await?;

        Ok(Self {
            mcp_server,
            task_ids_by_origin_step,
            latest_plan_step_statuses,
            derived_plan_step_events,
        })
    }

    fn task_id_for_origin_step(&self, step_id: Uuid) -> Option<Uuid> {
        self.task_ids_by_origin_step.get(&step_id).copied()
    }

    fn latest_plan_step_status(
        &self,
        plan_id: Uuid,
        step_id: Uuid,
        run_id: Uuid,
    ) -> Option<PlanStepStatus> {
        self.latest_plan_step_statuses
            .get(&(plan_id, step_id, run_id))
            .cloned()
    }

    fn has_derived_plan_step_event(&self, plan_id: Uuid, step_id: Uuid, run_id: Uuid) -> bool {
        self.derived_plan_step_events
            .contains(&(plan_id, step_id, run_id))
    }
}

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
        plan_id: run
            .plan()
            .map(|id| id.to_string())
            .or_else(|| run_binding.plan_id.clone()),
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
    let projection_context = PlanProjectionContext::load(storage_path).await?;
    let actor = projection_context
        .mcp_server
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

    let root_task_uuid = run_binding
        .task_id
        .parse::<Uuid>()
        .map_err(|error| anyhow!("invalid formal task id '{}': {error}", run_binding.task_id))?;
    let intent_uuid = plan.intent();
    let mut previous_step_task_id: Option<Uuid> = None;

    for (ordinal, step) in plan.steps().iter().enumerate() {
        let step_uuid = step.step_id();
        let step_id = step_uuid.to_string();
        let dependency_task_id = previous_step_task_id;
        let task_uuid = ensure_formal_plan_step_task(
            &projection_context,
            &actor,
            DerivedPlanStepTaskSpec {
                root_task_id: root_task_uuid,
                intent_id: intent_uuid,
                step_id: step_uuid,
                title: step.description(),
                dependency_task_id,
                plan_id: plan_uuid,
                run_id: run_uuid,
            },
        )
        .await?;
        previous_step_task_id = Some(task_uuid);

        let step_snapshot = PlanStepSnapshot {
            id: step_id.clone(),
            plan_id: plan_id.to_string(),
            text: step.description().to_string(),
            ordinal: ordinal as i64,
            created_at: plan.header().created_at(),
        };
        upsert_tracked_json_object(storage_path, "plan_step_snapshot", &step_id, &step_snapshot)
            .await?;

        // Keep a per-step task_snapshot projection aligned with the derived
        // formal Task objects so Codex-oriented history readers can inspect the
        // Claude bridge output without understanding the formal task schema.
        let task_snapshot_id = format!("task_{}_{}", plan_id, ordinal);
        let task_snapshot = TaskSnapshot {
            id: task_snapshot_id.clone(),
            thread_id: ai_session_id.to_string(),
            plan_id: Some(plan_id.to_string()),
            intent_id: Some(plan.intent().to_string()),
            turn_id: Some(ai_session_id.to_string()),
            title: Some(step.description().to_string()),
            parent_task_id: Some(run_binding.task_id.clone()),
            origin_step_id: Some(step_id.clone()),
            dependencies: dependency_task_id
                .map(|task_id| vec![task_id.to_string()])
                .unwrap_or_default(),
            created_at: plan.header().created_at(),
        };
        upsert_tracked_json_object(
            storage_path,
            "task_snapshot",
            &task_snapshot_id,
            &task_snapshot,
        )
        .await?;

        if !projection_context.has_derived_plan_step_event(plan_uuid, step_uuid, run_uuid) {
            let _ = projection_context
                .mcp_server
                .create_plan_step_event_impl(
                    CreatePlanStepEventParams {
                        plan_id: plan_uuid.to_string(),
                        step_id: step_uuid.to_string(),
                        run_id: run_uuid.to_string(),
                        status: "pending".to_string(),
                        reason: Some(DERIVED_PLAN_STEP_EVENT_REASON.to_string()),
                        consumed_frames: None,
                        produced_frames: None,
                        spawned_task_id: Some(task_uuid.to_string()),
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
    }

    Ok(())
}

async fn ensure_formal_plan_step_task(
    projection_context: &PlanProjectionContext,
    actor: &git_internal::internal::object::types::ActorRef,
    spec: DerivedPlanStepTaskSpec<'_>,
) -> Result<Uuid> {
    if let Some(existing_task_id) = projection_context.task_id_for_origin_step(spec.step_id) {
        return Ok(existing_task_id);
    }

    let status = projection_context
        .latest_plan_step_status(spec.plan_id, spec.step_id, spec.run_id)
        .map(task_status_for_plan_step_status)
        .unwrap_or("draft");
    let result = projection_context
        .mcp_server
        .create_task_impl(
            CreateTaskParams {
                title: spec.title.to_string(),
                description: Some(format!("Derived formal task for plan step: {}", spec.title)),
                goal_type: None,
                constraints: None,
                acceptance_criteria: None,
                requested_by_kind: None,
                requested_by_id: None,
                dependencies: spec
                    .dependency_task_id
                    .map(|task_id| vec![task_id.to_string()]),
                intent_id: Some(spec.intent_id.to_string()),
                parent_task_id: Some(spec.root_task_id.to_string()),
                origin_step_id: Some(spec.step_id.to_string()),
                status: Some(status.to_string()),
                reason: Some(DERIVED_PLAN_STEP_TASK_REASON.to_string()),
                tags: None,
                external_ids: None,
                actor_kind: Some("system".to_string()),
                actor_id: Some("claude-sdk-snapshot".to_string()),
            },
            actor.clone(),
        )
        .await
        .map_err(|error| anyhow!("failed to create derived formal task: {error:?}"))?;
    let task_id = parse_created_id("task", &result)?;
    task_id
        .parse::<Uuid>()
        .map_err(|error| anyhow!("invalid derived formal task id '{}': {error}", task_id))
}

async fn load_task_ids_by_origin_step(
    history: &HistoryManager,
    storage: &LocalStorage,
) -> Result<HashMap<Uuid, Uuid>> {
    let mut task_ids_by_origin_step = HashMap::new();
    for (object_id, object_hash) in history
        .list_objects("task")
        .await
        .context("failed to list task objects")?
    {
        let task = storage
            .get_json::<Task>(&object_hash)
            .await
            .with_context(|| format!("failed to read task '{object_id}'"))?;
        if let Some(step_id) = task.origin_step_id() {
            task_ids_by_origin_step
                .entry(step_id)
                .or_insert_with(|| task.header().object_id());
        }
    }
    Ok(task_ids_by_origin_step)
}

async fn load_plan_step_event_indexes(
    history: &HistoryManager,
    storage: &LocalStorage,
) -> Result<(
    HashMap<PlanStepEventKey, PlanStepStatus>,
    HashSet<PlanStepEventKey>,
)> {
    let mut latest_plan_step_statuses =
        HashMap::<PlanStepEventKey, (String, PlanStepStatus)>::new();
    let mut derived_plan_step_events = HashSet::new();
    for (object_id, object_hash) in history
        .list_objects("plan_step_event")
        .await
        .context("failed to list plan_step_event objects")?
    {
        let event = storage
            .get_json::<PlanStepEvent>(&object_hash)
            .await
            .with_context(|| format!("failed to read plan_step_event '{object_id}'"))?;
        let key = (event.plan_id(), event.step_id(), event.run_id());
        if event.reason() == Some(DERIVED_PLAN_STEP_EVENT_REASON) {
            derived_plan_step_events.insert(key);
        }

        let observed_at = event.header().created_at().to_string();
        let status = event.status().clone();
        match latest_plan_step_statuses.get_mut(&key) {
            Some((latest_observed_at, latest_status)) => {
                if observed_at > *latest_observed_at {
                    *latest_observed_at = observed_at;
                    *latest_status = status;
                }
            }
            None => {
                latest_plan_step_statuses.insert(key, (observed_at, status));
            }
        }
    }

    Ok((
        latest_plan_step_statuses
            .into_iter()
            .map(|(key, (_, status))| (key, status))
            .collect(),
        derived_plan_step_events,
    ))
}

fn task_status_for_plan_step_status(status: PlanStepStatus) -> &'static str {
    match status {
        PlanStepStatus::Pending => "draft",
        PlanStepStatus::Progressing => "running",
        PlanStepStatus::Completed => "done",
        PlanStepStatus::Failed => "failed",
        PlanStepStatus::Skipped => "cancelled",
    }
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
    let mut latest: Option<Provenance> = None;
    for (object_id, object_hash) in history
        .list_objects("provenance")
        .await
        .context("failed to list provenance objects")?
    {
        let provenance = storage
            .get_json::<Provenance>(&object_hash)
            .await
            .with_context(|| format!("failed to read provenance '{object_id}'"))?;
        if provenance.run_id().to_string() == run_id
            && latest.as_ref().is_none_or(|current| {
                provenance.header().created_at() >= current.header().created_at()
            })
        {
            latest = Some(provenance);
        }
    }
    if let Some(provenance) = latest {
        return Ok(provenance);
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
