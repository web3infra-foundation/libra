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
    },
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

/// Generic event wrapper; `payload` may embed domain-specific info (e.g., status string)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
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
        let record = EventRecord {
            id: id.to_string(),
            kind,
            status: status.into(),
            payload,
            created_at: Utc::now(),
        };
        super::store_to_mcp(&self.mcp, "event", id, &record, self.debug).await;
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

        if let Some(history) = &self.mcp.intent_history_manager
            && let Err(e) = history.append(object_type, object_id, hash).await
        {
            eprintln!("[WARN] Failed to append {object_type}/{object_id} to history: {e}");
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

        let mut thread = ThreadView::default();
        for intent in intents {
            thread.thread_id = intent.thread_id.clone();
            thread.intents.insert(intent.id.clone(), intent);
        }
        for plan in plans {
            thread.thread_id = plan.thread_id.clone();
            thread.plans.insert(plan.id.clone(), plan);
        }
        for step in plan_steps {
            thread.plan_steps.insert(step.id.clone(), step);
        }
        for task in tasks {
            thread.thread_id = task.thread_id.clone();
            thread.tasks.insert(task.id.clone(), task);
        }
        for run in runs {
            thread.thread_id = run.thread_id.clone();
            thread.runs.insert(run.id.clone(), run);
        }
        for patchset in patchsets {
            thread.thread_id = patchset.thread_id.clone();
            thread.patchsets.insert(patchset.id.clone(), patchset);
        }
        for snapshot in context_snapshots {
            thread
                .context_snapshots
                .insert(snapshot.id.clone(), snapshot);
        }
        for prov in provenance {
            thread.provenance.insert(prov.id.clone(), prov);
        }

        let mut intent_events_sorted = intent_events.clone();
        intent_events_sorted.sort_by_key(|e| e.at);

        let mut intent_heads: HashSet<String> = thread.intents.keys().cloned().collect();
        for event in &intent_events_sorted {
            if let Some(next_id) = event.next_intent_id.as_ref() {
                intent_heads.remove(next_id);
            }
        }
        thread.intent_heads = intent_heads.into_iter().collect();
        thread.intent_heads.sort();

        let latest_intent_id = thread
            .intents
            .values()
            .max_by_key(|i| i.created_at)
            .map(|i| i.id.clone());
        thread.current_intent_id = intent_events_sorted
            .last()
            .map(|e| e.intent_id.clone())
            .or_else(|| latest_intent_id.clone());
        thread.latest_intent_id = latest_intent_id;

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
        let mut task_events_sorted = task_events.clone();
        task_events_sorted.sort_by_key(|e| e.at);
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
        run_events_sorted.sort_by_key(|e| e.at);
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
        plan_step_events_sorted.sort_by_key(|e| e.at);
        for event in &plan_step_events_sorted {
            if event.status == "in_progress" {
                scheduler.active_plan_step_id = Some(event.step_id.clone());
            }
        }

        let mut plans_for_intent: Vec<&PlanSnapshot> = thread
            .plans
            .values()
            .filter(|p| p.intent_id.as_deref() == thread.current_intent_id.as_deref())
            .collect();
        plans_for_intent.sort_by_key(|p| p.created_at);
        if let Some(selected) = plans_for_intent.last() {
            scheduler.selected_plan_id = Some(selected.id.clone());
            scheduler.current_plan_heads = plans_for_intent.iter().map(|p| p.id.clone()).collect();
        } else {
            let mut all_plans: Vec<&PlanSnapshot> = thread.plans.values().collect();
            all_plans.sort_by_key(|p| p.created_at);
            if let Some(selected) = all_plans.last() {
                scheduler.selected_plan_id = Some(selected.id.clone());
                scheduler.current_plan_heads = all_plans.iter().map(|p| p.id.clone()).collect();
            }
        }

        let mut task_latest_status: HashMap<String, String> = HashMap::new();
        for event in &task_events_sorted {
            task_latest_status.insert(event.task_id.clone(), event.status.clone());
        }
        scheduler.ready_queue = thread
            .tasks
            .values()
            .filter(|task| {
                let status = task_latest_status.get(&task.id).map(|s| s.as_str());
                let matches_plan = scheduler
                    .selected_plan_id
                    .as_deref()
                    .map(|plan_id| task.plan_id.as_deref() == Some(plan_id))
                    .unwrap_or(true);
                matches_plan && status.is_none()
            })
            .map(|task| task.id.clone())
            .collect();

        let mut context_frames_sorted = context_frames.clone();
        context_frames_sorted.sort_by_key(|f| f.at);
        let mut live_context_window: Vec<ContextFrameEvent> = context_frames_sorted
            .into_iter()
            .rev()
            .take(LIVE_CONTEXT_WINDOW_MAX)
            .collect();
        live_context_window.reverse();
        scheduler.live_context_window = live_context_window.iter().map(|f| f.id.clone()).collect();

        ViewRebuildResult {
            thread,
            scheduler,
            index,
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
