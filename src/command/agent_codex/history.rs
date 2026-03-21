use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use git_internal::hash::ObjectHash;
use serde::{Deserialize, Serialize};

use super::{
    model::{
        ContextFrameEvent, ContextSnapshot, DecisionEvent, EvidenceEvent, IntentEvent,
        IntentSnapshot, PatchSetSnapshot, PlanSnapshot, PlanStepEvent, PlanStepSnapshot,
        ProvenanceSnapshot, RunEvent, RunSnapshot, RunUsage, TaskEvent, TaskSnapshot,
        ToolInvocationEvent,
    },
    types::{ToolInvocation, ToolStatus},
    view::{QueryIndex, SchedulerView, ThreadView, ViewRebuildResult},
};
use crate::{internal::ai::mcp::server::LibraMcpServer, utils::storage_ext::StorageExt};

const LIVE_CONTEXT_WINDOW_MAX: usize = 50;

/// Event kind taxonomy (simplified for codex notifications)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    RunStatus,
    TaskStatus,
    PlanStepStatus,
    ToolInvocationStatus,
    RunUsage,
    IntentStatus,
}

impl EventKind {
    fn storage_slug(&self) -> &'static str {
        match self {
            Self::RunStatus => "run_status",
            Self::TaskStatus => "task_status",
            Self::PlanStepStatus => "plan_step_status",
            Self::ToolInvocationStatus => "tool_invocation_status",
            Self::RunUsage => "run_usage",
            Self::IntentStatus => "intent_status",
        }
    }
}

/// Generic event wrapper; `payload` may embed domain-specific info (e.g., status string)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    #[serde(default)]
    pub subject_id: String,
    pub kind: EventKind,
    pub status: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Recorder abstraction; thin wrapper around MCP storage.
pub struct HistoryRecorder {
    pub mcp: Arc<LibraMcpServer>,
    pub debug: bool,
}

impl HistoryRecorder {
    pub fn new(mcp: Arc<LibraMcpServer>, debug: bool) -> Self {
        Self { mcp, debug }
    }

    /// Record an immutable snapshot object.
    pub async fn snapshot<T: Serialize + Send + Sync>(&self, kind: &str, id: &str, obj: &T) {
        super::store_to_mcp(&self.mcp, kind, id, obj, self.debug).await;
    }

    /// Record an event; wraps `EventRecord`.
    pub async fn event(
        &self,
        kind: EventKind,
        id: &str,
        status: impl Into<String>,
        payload: serde_json::Value,
    ) {
        let event_id = format!(
            "event_{}_{}_{}",
            kind.storage_slug(),
            id,
            Utc::now().timestamp_millis()
        );
        let record = EventRecord {
            id: event_id.clone(),
            subject_id: id.to_string(),
            kind,
            status: status.into(),
            payload,
            created_at: Utc::now(),
        };
        super::store_to_mcp(&self.mcp, "event", &event_id, &record, self.debug).await;
    }
}

/// Writes snapshots/events to MCP + git-internal with hash deduplication.
pub struct HistoryWriter {
    mcp: Arc<LibraMcpServer>,
    debug: bool,
    last_hashes: Mutex<HashMap<String, ObjectHash>>,
}

impl HistoryWriter {
    pub fn new(mcp: Arc<LibraMcpServer>, debug: bool) -> Self {
        Self {
            mcp,
            debug,
            last_hashes: Mutex::new(HashMap::new()),
        }
    }

    pub async fn write<T: Serialize + Send + Sync>(
        &self,
        object_type: &str,
        object_id: &str,
        obj: &T,
    ) {
        if object_id.is_empty() {
            eprintln!("[WARN] Refusing to append {object_type} with empty object id");
            return;
        }
        let Some(storage) = &self.mcp.storage else {
            eprintln!("[WARN] MCP storage not available");
            return;
        };

        let hash = match storage.put_json(obj).await {
            Ok(hash) => hash,
            Err(e) => {
                eprintln!("[WARN] Failed to store {object_type}: {e}");
                return;
            }
        };

        let key = format!("{}/{}", object_type, object_id);
        if let Ok(mut cache) = self.last_hashes.lock() {
            if let Some(prev) = cache.get(&key)
                && prev == &hash
            {
                return;
            }
            cache.insert(key, hash);
        }

        if let Err(e) =
            super::append_history_hash_if_changed(&self.mcp, object_type, object_id, hash).await
        {
            eprintln!("[WARN] {e}");
            return;
        }

        if self.debug {
            eprintln!("[DEBUG] Stored {object_type}/{object_id} (hash: {hash})");
        }
    }
}

/// Reads snapshots/events from git-internal via HistoryManager.
pub struct HistoryReader {
    mcp: Arc<LibraMcpServer>,
}

struct LatestThreadCandidates<'a> {
    intents: &'a [IntentSnapshot],
    plans: &'a [PlanSnapshot],
    tasks: &'a [TaskSnapshot],
    runs: &'a [RunSnapshot],
    patchsets: &'a [PatchSetSnapshot],
    context_snapshots: &'a [ContextSnapshot],
    run_usage: &'a [RunUsage],
    tool_invocation_events: &'a [ToolInvocationEvent],
}

impl HistoryReader {
    pub fn new(mcp: Arc<LibraMcpServer>) -> Self {
        Self { mcp }
    }

    async fn read_objects<T: for<'de> Deserialize<'de> + Send + Sync>(
        &self,
        object_type: &str,
    ) -> Vec<T> {
        let (Some(history), Some(storage)) = (&self.mcp.intent_history_manager, &self.mcp.storage)
        else {
            return Vec::new();
        };

        let Ok(items) = history.list_objects(object_type).await else {
            return Vec::new();
        };

        let mut out = Vec::with_capacity(items.len());
        for (_id, hash) in items {
            if let Ok(obj) = storage.get_json::<T>(&hash).await {
                out.push(obj);
            }
        }
        out
    }

    fn latest_thread_id(candidates: LatestThreadCandidates<'_>) -> Option<String> {
        let mut latest: Option<(String, DateTime<Utc>)> = None;

        let mut observe = |thread_id: &str, at: DateTime<Utc>| {
            if thread_id.is_empty() {
                return;
            }
            if latest.as_ref().is_none_or(|(_, prev)| at > *prev) {
                latest = Some((thread_id.to_string(), at));
            }
        };

        for intent in candidates.intents {
            observe(&intent.thread_id, intent.created_at);
        }
        for plan in candidates.plans {
            observe(&plan.thread_id, plan.created_at);
        }
        for task in candidates.tasks {
            observe(&task.thread_id, task.created_at);
        }
        for run in candidates.runs {
            observe(&run.thread_id, run.started_at);
        }
        for patchset in candidates.patchsets {
            observe(&patchset.thread_id, patchset.created_at);
        }
        for snapshot in candidates.context_snapshots {
            observe(&snapshot.thread_id, snapshot.created_at);
        }
        for usage in candidates.run_usage {
            observe(&usage.thread_id, usage.at);
        }
        for event in candidates.tool_invocation_events {
            observe(&event.thread_id, event.at);
        }

        latest.map(|(thread_id, _)| thread_id)
    }

    fn compute_intent_heads(
        intents: &HashMap<String, IntentSnapshot>,
        intent_events: &[IntentEvent],
    ) -> Vec<String> {
        let mut intent_heads: HashSet<String> = intents.keys().cloned().collect();
        for intent in intents.values() {
            for parent_id in &intent.parents {
                intent_heads.remove(parent_id);
            }
        }
        for event in intent_events {
            if event.next_intent_id.is_some() {
                intent_heads.remove(&event.intent_id);
            }
        }
        let mut heads: Vec<String> = intent_heads.into_iter().collect();
        heads.sort_by_key(|intent_id| intents.get(intent_id).map(|intent| intent.created_at));
        heads
    }

    fn compute_plan_heads<'a>(plans: impl IntoIterator<Item = &'a PlanSnapshot>) -> Vec<String> {
        let plans: Vec<&PlanSnapshot> = plans.into_iter().collect();
        let mut heads: HashSet<String> = plans.iter().map(|plan| plan.id.clone()).collect();
        for plan in &plans {
            for parent_id in &plan.parents {
                heads.remove(parent_id);
            }
        }
        let mut head_ids: Vec<String> = heads.into_iter().collect();
        head_ids.sort_by_key(|plan_id| {
            plans
                .iter()
                .find(|plan| plan.id == *plan_id)
                .map(|plan| plan.created_at)
        });
        head_ids
    }

    fn collapse_tool_invocations(events: &[ToolInvocationEvent]) -> Vec<ToolInvocation> {
        let mut sorted = events.to_vec();
        sorted.sort_by_key(|event| event.at);

        let mut by_id: HashMap<String, ToolInvocation> = HashMap::new();
        for event in sorted {
            let arguments = event
                .payload
                .get("arguments")
                .filter(|value| !value.is_null())
                .cloned();
            let result = event
                .payload
                .get("result")
                .filter(|value| !value.is_null())
                .cloned();
            let error = event
                .payload
                .get("error")
                .and_then(|value| value.as_str())
                .map(String::from);
            let duration_ms = event
                .payload
                .get("duration_ms")
                .and_then(|value| value.as_i64());

            let entry = by_id
                .entry(event.id.clone())
                .or_insert_with(|| ToolInvocation {
                    id: event.id.clone(),
                    run_id: event.run_id.clone(),
                    thread_id: event.thread_id.clone(),
                    tool_name: event.tool.clone(),
                    server: event.server.clone(),
                    arguments: arguments.clone(),
                    result: result.clone(),
                    error: error.clone(),
                    status: tool_status_from_str(&event.status),
                    duration_ms,
                    created_at: event.at,
                });

            entry.run_id = event.run_id.clone();
            entry.thread_id = event.thread_id.clone();
            entry.tool_name = event.tool.clone();
            entry.server = event.server.clone();
            if arguments.is_some() {
                entry.arguments = arguments;
            }
            if result.is_some() {
                entry.result = result;
            }
            if error.is_some() {
                entry.error = error;
            }
            if duration_ms.is_some() {
                entry.duration_ms = duration_ms;
            }
            entry.status = tool_status_from_str(&event.status);
        }

        let mut invocations: Vec<ToolInvocation> = by_id.into_values().collect();
        invocations.sort_by_key(|invocation| invocation.created_at);
        invocations
    }

    pub async fn rebuild_view(&self) -> ViewRebuildResult {
        let intents = self.read_objects::<IntentSnapshot>("intent_snapshot").await;
        let plans = self.read_objects::<PlanSnapshot>("plan_snapshot").await;
        let plan_steps = self
            .read_objects::<PlanStepSnapshot>("plan_step_snapshot")
            .await;
        let tasks = self.read_objects::<TaskSnapshot>("task_snapshot").await;
        let runs = self.read_objects::<RunSnapshot>("run_snapshot").await;
        let patchsets = self
            .read_objects::<PatchSetSnapshot>("patchset_snapshot")
            .await;
        let context_snapshots = self
            .read_objects::<ContextSnapshot>("context_snapshot")
            .await;
        let provenance = self
            .read_objects::<ProvenanceSnapshot>("provenance_snapshot")
            .await;

        let intent_events = self.read_objects::<IntentEvent>("intent_event").await;
        let task_events = self.read_objects::<TaskEvent>("task_event").await;
        let run_events = self.read_objects::<RunEvent>("run_event").await;
        let plan_step_events = self.read_objects::<PlanStepEvent>("plan_step_event").await;
        let run_usage = self.read_objects::<RunUsage>("run_usage").await;
        let evidence = self.read_objects::<EvidenceEvent>("evidence").await;
        let decisions = self.read_objects::<DecisionEvent>("decision").await;
        let context_frames = self
            .read_objects::<ContextFrameEvent>("context_frame")
            .await;
        let tool_invocation_events = self
            .read_objects::<ToolInvocationEvent>("tool_invocation_event")
            .await;

        let selected_thread_id = Self::latest_thread_id(LatestThreadCandidates {
            intents: &intents,
            plans: &plans,
            tasks: &tasks,
            runs: &runs,
            patchsets: &patchsets,
            context_snapshots: &context_snapshots,
            run_usage: &run_usage,
            tool_invocation_events: &tool_invocation_events,
        });

        let mut thread = ThreadView {
            thread_id: selected_thread_id.clone().unwrap_or_default(),
            ..ThreadView::default()
        };

        for intent in intents.into_iter().filter(|intent| {
            selected_thread_id
                .as_deref()
                .is_none_or(|id| intent.thread_id == id)
        }) {
            thread.intents.insert(intent.id.clone(), intent);
        }
        for plan in plans.into_iter().filter(|plan| {
            selected_thread_id
                .as_deref()
                .is_none_or(|id| plan.thread_id == id)
        }) {
            thread.plans.insert(plan.id.clone(), plan);
        }
        for task in tasks.into_iter().filter(|task| {
            selected_thread_id
                .as_deref()
                .is_none_or(|id| task.thread_id == id)
        }) {
            thread.tasks.insert(task.id.clone(), task);
        }
        for run in runs.into_iter().filter(|run| {
            selected_thread_id
                .as_deref()
                .is_none_or(|id| run.thread_id == id)
        }) {
            thread.runs.insert(run.id.clone(), run);
        }
        for patchset in patchsets.into_iter().filter(|patchset| {
            selected_thread_id
                .as_deref()
                .is_none_or(|id| patchset.thread_id == id)
        }) {
            thread.patchsets.insert(patchset.id.clone(), patchset);
        }
        for snapshot in context_snapshots.into_iter().filter(|snapshot| {
            selected_thread_id
                .as_deref()
                .is_none_or(|id| snapshot.thread_id == id)
                || snapshot
                    .run_id
                    .as_ref()
                    .is_some_and(|run_id| thread.runs.contains_key(run_id))
        }) {
            thread
                .context_snapshots
                .insert(snapshot.id.clone(), snapshot);
        }
        for prov in provenance
            .into_iter()
            .filter(|prov| thread.runs.contains_key(&prov.run_id))
        {
            thread.provenance.insert(prov.id.clone(), prov);
        }

        for step in plan_steps
            .into_iter()
            .filter(|step| thread.plans.contains_key(&step.plan_id))
        {
            thread.plan_steps.insert(step.id.clone(), step);
        }

        let thread_intent_ids: HashSet<String> = thread.intents.keys().cloned().collect();
        let thread_task_ids: HashSet<String> = thread.tasks.keys().cloned().collect();
        let thread_plan_ids: HashSet<String> = thread.plans.keys().cloned().collect();
        let thread_run_ids: HashSet<String> = thread.runs.keys().cloned().collect();
        let thread_patchset_ids: HashSet<String> = thread.patchsets.keys().cloned().collect();

        let intent_events: Vec<IntentEvent> = intent_events
            .into_iter()
            .filter(|event| thread_intent_ids.contains(&event.intent_id))
            .collect();
        let task_events: Vec<TaskEvent> = task_events
            .into_iter()
            .filter(|event| thread_task_ids.contains(&event.task_id))
            .collect();
        let run_events: Vec<RunEvent> = run_events
            .into_iter()
            .filter(|event| thread_run_ids.contains(&event.run_id))
            .collect();
        let plan_step_events: Vec<PlanStepEvent> = plan_step_events
            .into_iter()
            .filter(|event| {
                thread_plan_ids.contains(&event.plan_id)
                    || thread_run_ids.contains(&event.run_id.clone().unwrap_or_default())
            })
            .collect();
        let run_usage: Vec<RunUsage> = run_usage
            .into_iter()
            .filter(|usage| {
                thread_run_ids.contains(&usage.run_id)
                    || selected_thread_id
                        .as_deref()
                        .is_some_and(|id| usage.thread_id == id)
            })
            .collect();
        let evidence: Vec<EvidenceEvent> = evidence
            .into_iter()
            .filter(|event| {
                thread_run_ids.contains(&event.run_id)
                    || event
                        .patchset_id
                        .as_ref()
                        .is_some_and(|patchset_id| thread_patchset_ids.contains(patchset_id))
            })
            .collect();
        let decisions: Vec<DecisionEvent> = decisions
            .into_iter()
            .filter(|event| {
                thread_run_ids.contains(&event.run_id)
                    || event
                        .chosen_patchset_id
                        .as_ref()
                        .is_some_and(|patchset_id| thread_patchset_ids.contains(patchset_id))
            })
            .collect();
        let context_frames: Vec<ContextFrameEvent> = context_frames
            .into_iter()
            .filter(|event| thread_run_ids.contains(&event.run_id))
            .collect();
        let tool_invocation_events: Vec<ToolInvocationEvent> = tool_invocation_events
            .into_iter()
            .filter(|event| {
                thread_run_ids.contains(&event.run_id)
                    || selected_thread_id
                        .as_deref()
                        .is_some_and(|id| event.thread_id == id)
            })
            .collect();
        let tool_invocations = Self::collapse_tool_invocations(&tool_invocation_events);

        let mut intent_events_sorted = intent_events.clone();
        intent_events_sorted.sort_by_key(|event| event.at);

        thread.intent_heads = Self::compute_intent_heads(&thread.intents, &intent_events_sorted);
        let latest_intent_id = thread
            .intents
            .values()
            .max_by_key(|intent| intent.created_at)
            .map(|intent| intent.id.clone());
        thread.latest_intent_id = latest_intent_id.clone();
        thread.current_intent_id = thread
            .intent_heads
            .iter()
            .filter_map(|intent_id| thread.intents.get(intent_id))
            .max_by_key(|intent| intent.created_at)
            .map(|intent| intent.id.clone())
            .or_else(|| latest_intent_id.clone());
        thread.updated_at = thread
            .runs
            .values()
            .map(|run| run.started_at)
            .chain(
                thread
                    .patchsets
                    .values()
                    .map(|patchset| patchset.created_at),
            )
            .chain(thread.tasks.values().map(|task| task.created_at))
            .chain(thread.plans.values().map(|plan| plan.created_at))
            .chain(thread.intents.values().map(|intent| intent.created_at))
            .max();

        let mut index = QueryIndex::default();
        for task in thread.tasks.values() {
            if let Some(plan_id) = task.plan_id.as_ref() {
                index
                    .plan_task_ids
                    .entry(plan_id.clone())
                    .or_default()
                    .push(task.id.clone());
            }
        }
        let mut latest_run_by_task: HashMap<String, (String, DateTime<Utc>)> = HashMap::new();
        for run in thread.runs.values() {
            if let Some(task_id) = run.task_id.as_ref() {
                index
                    .task_run_ids
                    .entry(task_id.clone())
                    .or_default()
                    .push(run.id.clone());
                let entry = latest_run_by_task
                    .entry(task_id.clone())
                    .or_insert((run.id.clone(), run.started_at));
                if run.started_at > entry.1 {
                    *entry = (run.id.clone(), run.started_at);
                }
            }
        }

        let mut task_events_sorted = task_events.clone();
        task_events_sorted.sort_by_key(|event| event.at);
        for event in &task_events_sorted {
            if let Some(run_id) = event.run_id.as_ref()
                && thread_run_ids.contains(run_id)
            {
                let runs_for_task = index.task_run_ids.entry(event.task_id.clone()).or_default();
                if !runs_for_task.contains(run_id) {
                    runs_for_task.push(run_id.clone());
                }
                let observed_at = thread
                    .runs
                    .get(run_id)
                    .map(|run| run.started_at)
                    .unwrap_or(event.at);
                let entry = latest_run_by_task
                    .entry(event.task_id.clone())
                    .or_insert((run_id.clone(), observed_at));
                if observed_at > entry.1 {
                    *entry = (run_id.clone(), observed_at);
                }
            }
        }
        for (task_id, (run_id, _)) in latest_run_by_task {
            index.task_latest_run_id.insert(task_id, run_id);
        }

        let mut latest_patchset_by_run: HashMap<String, (String, DateTime<Utc>)> = HashMap::new();
        for patchset in thread.patchsets.values() {
            let entry = latest_patchset_by_run
                .entry(patchset.run_id.clone())
                .or_insert((patchset.id.clone(), patchset.created_at));
            if patchset.created_at > entry.1 {
                *entry = (patchset.id.clone(), patchset.created_at);
            }
        }
        for (run_id, (patchset_id, _)) in latest_patchset_by_run {
            index.run_latest_patchset_id.insert(run_id, patchset_id);
        }

        let mut scheduler = SchedulerView::default();
        for event in &task_events_sorted {
            if event.status == "in_progress" {
                scheduler.active_task_id = Some(event.task_id.clone());
            }
            if (event.status == "completed" || event.status == "failed")
                && scheduler.active_task_id.as_deref() == Some(&event.task_id)
            {
                scheduler.active_task_id = None;
            }
        }

        let mut run_events_sorted = run_events.clone();
        run_events_sorted.sort_by_key(|event| event.at);
        for event in &run_events_sorted {
            if event.status == "in_progress" {
                scheduler.active_run_id = Some(event.run_id.clone());
            }
            if (event.status == "completed" || event.status == "failed")
                && scheduler.active_run_id.as_deref() == Some(&event.run_id)
            {
                scheduler.active_run_id = None;
            }
        }

        let mut plan_step_events_sorted = plan_step_events.clone();
        plan_step_events_sorted.sort_by_key(|event| event.at);
        for event in &plan_step_events_sorted {
            if event.status == "in_progress" {
                scheduler.active_plan_step_id = Some(event.step_id.clone());
            }
        }

        let mut plans_for_intent: Vec<&PlanSnapshot> = thread
            .plans
            .values()
            .filter(|plan| plan.intent_id.as_deref() == thread.current_intent_id.as_deref())
            .collect();
        plans_for_intent.sort_by_key(|plan| plan.created_at);
        if plans_for_intent.is_empty() {
            plans_for_intent = thread.plans.values().collect();
            plans_for_intent.sort_by_key(|plan| plan.created_at);
        }
        scheduler.current_plan_heads = Self::compute_plan_heads(plans_for_intent.iter().copied());
        scheduler.selected_plan_id = scheduler
            .current_plan_heads
            .iter()
            .filter_map(|plan_id| thread.plans.get(plan_id))
            .max_by_key(|plan| plan.created_at)
            .map(|plan| plan.id.clone());

        let mut task_latest_status: HashMap<String, String> = HashMap::new();
        for event in &task_events_sorted {
            task_latest_status.insert(event.task_id.clone(), event.status.clone());
        }
        scheduler.ready_queue = thread
            .tasks
            .values()
            .filter(|task| {
                let status = task_latest_status
                    .get(&task.id)
                    .map(|status| status.as_str());
                let matches_plan = scheduler
                    .selected_plan_id
                    .as_deref()
                    .map(|plan_id| task.plan_id.as_deref() == Some(plan_id))
                    .unwrap_or(true);
                matches_plan && matches!(status, None | Some("pending"))
            })
            .map(|task| task.id.clone())
            .collect();

        let mut context_frames_sorted = context_frames.clone();
        context_frames_sorted.sort_by_key(|frame| frame.at);
        let mut live_context_window: Vec<ContextFrameEvent> = context_frames_sorted
            .into_iter()
            .rev()
            .take(LIVE_CONTEXT_WINDOW_MAX)
            .collect();
        live_context_window.reverse();
        scheduler.live_context_window = live_context_window
            .iter()
            .map(|frame| frame.id.clone())
            .collect();
        scheduler.updated_at = run_events_sorted
            .last()
            .map(|event| event.at)
            .or_else(|| task_events_sorted.last().map(|event| event.at))
            .or_else(|| plan_step_events_sorted.last().map(|event| event.at));

        ViewRebuildResult {
            thread,
            scheduler,
            index,
            tool_invocations,
            intent_events,
            task_events,
            run_events,
            plan_step_events,
            run_usage,
            evidence,
            decisions,
            context_frames,
        }
    }
}

fn tool_status_from_str(status: &str) -> ToolStatus {
    match status {
        "completed" => ToolStatus::Completed,
        "failed" => ToolStatus::Failed,
        "in_progress" | "started" => ToolStatus::InProgress,
        _ => ToolStatus::Pending,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::{
        internal::ai::{history::HistoryManager, mcp::server::LibraMcpServer},
        utils::{storage::local::LocalStorage, test},
    };

    #[test]
    fn compute_intent_heads_removes_superseded_nodes() {
        let intent_1 = IntentSnapshot {
            id: "intent-1".to_string(),
            content: "first".to_string(),
            thread_id: "thread-1".to_string(),
            parents: Vec::new(),
            analysis_context_frames: Vec::new(),
            created_at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 0, 0).unwrap(),
        };
        let intent_2 = IntentSnapshot {
            id: "intent-2".to_string(),
            content: "second".to_string(),
            thread_id: "thread-1".to_string(),
            parents: vec!["intent-1".to_string()],
            analysis_context_frames: Vec::new(),
            created_at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 1, 0).unwrap(),
        };

        let intents = HashMap::from([
            (intent_1.id.clone(), intent_1),
            (intent_2.id.clone(), intent_2),
        ]);
        let intent_events = vec![IntentEvent {
            id: "intent-link".to_string(),
            intent_id: "intent-1".to_string(),
            status: "continued".to_string(),
            at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 1, 30).unwrap(),
            next_intent_id: Some("intent-2".to_string()),
        }];

        let heads = HistoryReader::compute_intent_heads(&intents, &intent_events);
        assert_eq!(heads, vec!["intent-2".to_string()]);
    }

    #[test]
    fn collapse_tool_invocations_merges_latest_payload() {
        let started_at = Utc.with_ymd_and_hms(2026, 3, 21, 11, 0, 0).unwrap();
        let completed_at = Utc.with_ymd_and_hms(2026, 3, 21, 11, 0, 2).unwrap();
        let events = vec![
            ToolInvocationEvent {
                id: "call-1".to_string(),
                run_id: "run-1".to_string(),
                thread_id: "thread-1".to_string(),
                tool: "shell".to_string(),
                server: None,
                status: "in_progress".to_string(),
                at: started_at,
                payload: serde_json::json!({
                    "arguments": {"command": "git status"},
                    "result": null,
                    "error": null,
                    "duration_ms": null,
                }),
            },
            ToolInvocationEvent {
                id: "call-1".to_string(),
                run_id: "run-1".to_string(),
                thread_id: "thread-1".to_string(),
                tool: "shell".to_string(),
                server: None,
                status: "completed".to_string(),
                at: completed_at,
                payload: serde_json::json!({
                    "result": {"output": "clean"},
                    "error": null,
                    "duration_ms": 42,
                }),
            },
        ];

        let invocations = HistoryReader::collapse_tool_invocations(&events);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].id, "call-1");
        assert_eq!(invocations[0].status, ToolStatus::Completed);
        assert_eq!(
            invocations[0].arguments,
            Some(serde_json::json!({"command": "git status"}))
        );
        assert_eq!(
            invocations[0].result,
            Some(serde_json::json!({"output": "clean"}))
        );
        assert_eq!(invocations[0].duration_ms, Some(42));
    }

    #[tokio::test]
    async fn rebuild_view_scopes_to_latest_thread_and_derives_run_links() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(dir.path());
        test::setup_with_new_libra_in(dir.path()).await;

        let libra_dir = dir.path().join(".libra");
        let storage = Arc::new(LocalStorage::new(libra_dir.join("objects")));
        let db_conn = Arc::new(
            crate::internal::db::establish_connection(
                libra_dir
                    .join("libra.db")
                    .to_str()
                    .expect("db path should be valid UTF-8"),
            )
            .await
            .expect("failed to connect test database"),
        );
        let history = Arc::new(HistoryManager::new(storage.clone(), libra_dir, db_conn));
        let mcp = Arc::new(LibraMcpServer::new(Some(history), Some(storage)));
        let writer = HistoryWriter::new(mcp.clone(), false);

        writer
            .write(
                "intent_snapshot",
                "intent-a",
                &IntentSnapshot {
                    id: "intent-a".to_string(),
                    content: "old thread".to_string(),
                    thread_id: "thread-a".to_string(),
                    parents: Vec::new(),
                    analysis_context_frames: Vec::new(),
                    created_at: Utc.with_ymd_and_hms(2026, 3, 21, 9, 0, 0).unwrap(),
                },
            )
            .await;
        writer
            .write(
                "task_snapshot",
                "task-a",
                &TaskSnapshot {
                    id: "task-a".to_string(),
                    thread_id: "thread-a".to_string(),
                    plan_id: None,
                    intent_id: Some("intent-a".to_string()),
                    turn_id: Some("run-a".to_string()),
                    title: Some("older task".to_string()),
                    parent_task_id: None,
                    origin_step_id: None,
                    dependencies: Vec::new(),
                    created_at: Utc.with_ymd_and_hms(2026, 3, 21, 9, 1, 0).unwrap(),
                },
            )
            .await;

        writer
            .write(
                "intent_snapshot",
                "intent-b",
                &IntentSnapshot {
                    id: "intent-b".to_string(),
                    content: "latest thread".to_string(),
                    thread_id: "thread-b".to_string(),
                    parents: vec!["intent-a".to_string()],
                    analysis_context_frames: Vec::new(),
                    created_at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 0, 0).unwrap(),
                },
            )
            .await;
        writer
            .write(
                "plan_snapshot",
                "plan-b",
                &PlanSnapshot {
                    id: "plan-b".to_string(),
                    thread_id: "thread-b".to_string(),
                    intent_id: Some("intent-b".to_string()),
                    turn_id: Some("run-b".to_string()),
                    step_text: "inspect".to_string(),
                    parents: Vec::new(),
                    context_frames: Vec::new(),
                    created_at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 0, 10).unwrap(),
                },
            )
            .await;
        writer
            .write(
                "task_snapshot",
                "task-b",
                &TaskSnapshot {
                    id: "task-b".to_string(),
                    thread_id: "thread-b".to_string(),
                    plan_id: Some("plan-b".to_string()),
                    intent_id: Some("intent-b".to_string()),
                    turn_id: Some("run-b".to_string()),
                    title: Some("latest task".to_string()),
                    parent_task_id: None,
                    origin_step_id: Some("plan-b".to_string()),
                    dependencies: Vec::new(),
                    created_at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 0, 20).unwrap(),
                },
            )
            .await;
        writer
            .write(
                "run_snapshot",
                "run-b",
                &RunSnapshot {
                    id: "run-b".to_string(),
                    thread_id: "thread-b".to_string(),
                    plan_id: None,
                    task_id: None,
                    started_at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 0, 30).unwrap(),
                },
            )
            .await;
        writer
            .write(
                "task_event",
                "task-event-b",
                &TaskEvent {
                    id: "task-event-b".to_string(),
                    task_id: "task-b".to_string(),
                    status: "in_progress".to_string(),
                    at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 0, 31).unwrap(),
                    run_id: Some("run-b".to_string()),
                },
            )
            .await;
        writer
            .write(
                "tool_invocation_event",
                "tool-invocation-event-b",
                &ToolInvocationEvent {
                    id: "call-b".to_string(),
                    run_id: "run-b".to_string(),
                    thread_id: "thread-b".to_string(),
                    tool: "shell".to_string(),
                    server: None,
                    status: "completed".to_string(),
                    at: Utc.with_ymd_and_hms(2026, 3, 21, 10, 0, 32).unwrap(),
                    payload: serde_json::json!({
                        "arguments": {"command": "cargo check"},
                        "result": {"output": "ok"},
                        "duration_ms": 12,
                    }),
                },
            )
            .await;

        let reader = HistoryReader::new(mcp);
        let rebuild = reader.rebuild_view().await;

        assert_eq!(rebuild.thread.thread_id, "thread-b");
        assert!(rebuild.thread.tasks.contains_key("task-b"));
        assert!(!rebuild.thread.tasks.contains_key("task-a"));
        assert_eq!(
            rebuild.index.task_latest_run_id.get("task-b"),
            Some(&"run-b".to_string())
        );
        assert_eq!(rebuild.tool_invocations.len(), 1);
        assert_eq!(rebuild.tool_invocations[0].id, "call-b");
        assert_eq!(rebuild.tool_invocations[0].status, ToolStatus::Completed);
    }
}
