//! Thread graph TUI for inspecting AI workflow version state.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Color, Line, Modifier, Span, Style, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use unicode_width::UnicodeWidthChar;
use uuid::Uuid;

use crate::{
    internal::{
        ai::{
            history::HistoryManager,
            projection::{ProjectionRebuilder, ProjectionResolver, ThreadBundle},
        },
        db::establish_connection,
        model::{
            ai_index_intent_plan, ai_index_intent_task, ai_index_plan_step_task,
            ai_index_run_event, ai_index_run_patchset, ai_index_task_run, ai_thread_intent,
        },
        tui::{Tui, tui_init, tui_restore},
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        storage::{Storage, local::LocalStorage},
        util::{DATABASE, try_get_storage_path},
    },
};

const MAX_OBJECT_DETAIL_LINES: usize = 160;
const MAX_OBJECT_DETAIL_LINE_CHARS: usize = 240;
const MIN_DETAIL_VALUE_WIDTH: usize = 12;
const GRAPH_PANE_WEIGHT: u32 = 1;
const DETAILS_PANE_WEIGHT: u32 = 2;

/// Command-line arguments for `libra graph`.
#[derive(Parser, Debug)]
pub struct GraphArgs {
    /// Canonical Libra Thread ID to inspect.
    pub thread_id: String,

    /// Path to a Libra repository to inspect instead of discovering one from the current directory.
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

/// Execute `libra graph`.
pub async fn execute_safe(args: GraphArgs, _output: &OutputConfig) -> CliResult<()> {
    let requested_thread_id = Uuid::parse_str(&args.thread_id).map_err(|error| {
        CliError::command_usage(format!(
            "graph expects a canonical thread_id UUID (got '{}': {error})",
            args.thread_id
        ))
    })?;

    let storage_root = try_get_storage_path(args.repo.clone()).map_err(|error| {
        CliError::repo_not_found()
            .with_hint(format!("failed to resolve repository storage: {error}"))
    })?;

    let graph = load_thread_graph(&storage_root, requested_thread_id)
        .await
        .map_err(|error| {
            CliError::fatal(format!(
                "failed to load thread graph for '{}': {error:#}",
                args.thread_id
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
            .with_hint("run `libra code` first so the thread projection can be recorded.")
        })?;

    run_graph_tui(graph).map_err(|error| {
        CliError::io(format!("failed to run graph TUI: {error}"))
            .with_hint("run this command from an interactive terminal.")
    })?;

    Ok(())
}

async fn load_thread_graph(storage_root: &Path, requested_thread_id: Uuid) -> Result<ThreadGraph> {
    let db_path = storage_root.join(DATABASE);
    let db_path_str = db_path.to_str().ok_or_else(|| {
        anyhow::anyhow!("database path is not valid UTF-8: {}", db_path.display())
    })?;
    let db_conn = establish_connection(db_path_str)
        .await
        .with_context(|| format!("failed to open repository database '{}'", db_path.display()))?;
    let storage = std::sync::Arc::new(LocalStorage::new(storage_root.join("objects")));
    let history = HistoryManager::new(
        storage.clone(),
        storage_root.to_path_buf(),
        std::sync::Arc::new(db_conn.clone()),
    );
    let rebuilder = ProjectionRebuilder::new(storage.as_ref(), &history);
    let resolver = ProjectionResolver::new(db_conn.clone());

    let bundle =
        load_bundle_for_graph(&db_conn, &resolver, &rebuilder, requested_thread_id).await?;
    let rows = load_projection_index_rows(&db_conn, &bundle).await?;
    let object_details =
        load_graph_object_details(&history, storage.as_ref(), &bundle, &rows).await;
    Ok(ThreadGraph::from_projection(bundle, rows, object_details))
}

async fn load_bundle_for_graph(
    db_conn: &DatabaseConnection,
    resolver: &ProjectionResolver,
    rebuilder: &ProjectionRebuilder<'_>,
    requested_thread_id: Uuid,
) -> Result<ThreadBundle> {
    if let Some(bundle) = resolver
        .load_or_rebuild_thread_bundle(requested_thread_id, rebuilder)
        .await
        .with_context(|| format!("failed to load projection for thread {requested_thread_id}"))?
    {
        return Ok(bundle);
    }

    if let Some(thread_id) =
        resolve_thread_id_from_intent_index(db_conn, requested_thread_id).await?
        && let Some(bundle) = resolver
            .load_or_rebuild_thread_bundle(thread_id, rebuilder)
            .await
            .with_context(|| {
                format!("failed to load projection for thread {thread_id} from intent index")
            })?
    {
        return Ok(bundle);
    }

    if let Some(rebuild) = rebuilder
        .materialize_latest_thread(db_conn)
        .await
        .context("failed to rebuild latest AI thread projection")?
        && (rebuild.thread.thread_id == requested_thread_id
            || rebuild
                .thread
                .intents
                .iter()
                .any(|intent| intent.intent_id == requested_thread_id))
        && let Some(bundle) = resolver
            .load_thread_bundle(rebuild.thread.thread_id)
            .await
            .with_context(|| {
                format!(
                    "failed to load rebuilt projection for thread {}",
                    rebuild.thread.thread_id
                )
            })?
    {
        return Ok(bundle);
    }

    bail!(
        "no thread projection or AI history was found for '{}'",
        requested_thread_id
    )
}

async fn resolve_thread_id_from_intent_index(
    db_conn: &DatabaseConnection,
    intent_id: Uuid,
) -> Result<Option<Uuid>> {
    let Some(row) = ai_thread_intent::Entity::find()
        .filter(ai_thread_intent::Column::IntentId.eq(intent_id.to_string()))
        .one(db_conn)
        .await
        .with_context(|| format!("failed to query thread membership for intent {intent_id}"))?
    else {
        return Ok(None);
    };

    Uuid::parse_str(&row.thread_id)
        .map(Some)
        .with_context(|| format!("invalid thread_id '{}' in ai_thread_intent", row.thread_id))
}

#[derive(Debug, Clone, Default)]
struct ProjectionIndexRows {
    intent_plans: Vec<ai_index_intent_plan::Model>,
    intent_tasks: Vec<ai_index_intent_task::Model>,
    plan_tasks: Vec<ai_index_plan_step_task::Model>,
    task_runs: Vec<ai_index_task_run::Model>,
    run_events: Vec<ai_index_run_event::Model>,
    run_patchsets: Vec<ai_index_run_patchset::Model>,
}

async fn load_projection_index_rows(
    db_conn: &DatabaseConnection,
    bundle: &ThreadBundle,
) -> Result<ProjectionIndexRows> {
    let intent_ids = bundle
        .thread
        .intents
        .iter()
        .map(|intent| intent.intent_id.to_string())
        .collect::<Vec<_>>();
    if intent_ids.is_empty() {
        return Ok(ProjectionIndexRows::default());
    }

    let intent_plans = ai_index_intent_plan::Entity::find()
        .filter(ai_index_intent_plan::Column::IntentId.is_in(intent_ids.clone()))
        .order_by_asc(ai_index_intent_plan::Column::CreatedAt)
        .all(db_conn)
        .await
        .context("failed to load intent -> plan index rows")?;
    let intent_tasks = ai_index_intent_task::Entity::find()
        .filter(ai_index_intent_task::Column::IntentId.is_in(intent_ids))
        .order_by_asc(ai_index_intent_task::Column::CreatedAt)
        .all(db_conn)
        .await
        .context("failed to load intent -> task index rows")?;

    let plan_ids = intent_plans
        .iter()
        .map(|row| row.plan_id.clone())
        .collect::<Vec<_>>();
    let plan_tasks = if plan_ids.is_empty() {
        Vec::new()
    } else {
        ai_index_plan_step_task::Entity::find()
            .filter(ai_index_plan_step_task::Column::PlanId.is_in(plan_ids))
            .order_by_asc(ai_index_plan_step_task::Column::CreatedAt)
            .all(db_conn)
            .await
            .context("failed to load plan step -> task index rows")?
    };

    let task_ids = intent_tasks
        .iter()
        .map(|row| row.task_id.clone())
        .chain(plan_tasks.iter().map(|row| row.task_id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let task_runs = if task_ids.is_empty() {
        Vec::new()
    } else {
        ai_index_task_run::Entity::find()
            .filter(ai_index_task_run::Column::TaskId.is_in(task_ids))
            .order_by_asc(ai_index_task_run::Column::CreatedAt)
            .all(db_conn)
            .await
            .context("failed to load task -> run index rows")?
    };

    let run_ids = task_runs
        .iter()
        .map(|row| row.run_id.clone())
        .collect::<Vec<_>>();
    let run_events = if run_ids.is_empty() {
        Vec::new()
    } else {
        ai_index_run_event::Entity::find()
            .filter(ai_index_run_event::Column::RunId.is_in(run_ids.clone()))
            .order_by_asc(ai_index_run_event::Column::CreatedAt)
            .all(db_conn)
            .await
            .context("failed to load run -> event index rows")?
    };
    let run_patchsets = if run_ids.is_empty() {
        Vec::new()
    } else {
        ai_index_run_patchset::Entity::find()
            .filter(ai_index_run_patchset::Column::RunId.is_in(run_ids))
            .order_by_asc(ai_index_run_patchset::Column::Sequence)
            .all(db_conn)
            .await
            .context("failed to load run -> patchset index rows")?
    };

    Ok(ProjectionIndexRows {
        intent_plans,
        intent_tasks,
        plan_tasks,
        task_runs,
        run_events,
        run_patchsets,
    })
}

#[derive(Debug, Clone)]
struct ThreadGraph {
    thread_id: Uuid,
    title: Option<String>,
    freshness: String,
    thread_version: i64,
    scheduler_version: i64,
    updated_at: DateTime<Utc>,
    selected_plan_id: Option<Uuid>,
    active_task_id: Option<Uuid>,
    active_run_id: Option<Uuid>,
    lines: Vec<GraphLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphLine {
    depth: usize,
    kind: GraphNodeKind,
    id: String,
    label: String,
    tags: Vec<String>,
    detail: Vec<(String, String)>,
    object: Option<GraphObjectDetail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GraphNodeKind {
    Intent,
    Plan,
    Task,
    Run,
    Patchset,
}

impl GraphNodeKind {
    fn marker(self) -> &'static str {
        match self {
            Self::Intent => "I",
            Self::Plan => "P",
            Self::Task => "T",
            Self::Run => "R",
            Self::Patchset => "D",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Intent => "Intent",
            Self::Plan => "Plan",
            Self::Task => "Task",
            Self::Run => "Run",
            Self::Patchset => "PatchSet",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Intent => Color::Cyan,
            Self::Plan => Color::Yellow,
            Self::Task => Color::Green,
            Self::Run => Color::Magenta,
            Self::Patchset => Color::Blue,
        }
    }

    fn history_type(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Plan => "plan",
            Self::Task => "task",
            Self::Run => "run",
            Self::Patchset => "patchset",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphObjectDetail {
    object_type: String,
    hash: Option<String>,
    git_object_type: Option<String>,
    summary: Vec<(String, String)>,
    raw_json_lines: Vec<String>,
}

impl GraphObjectDetail {
    fn from_json(
        kind: GraphNodeKind,
        hash: Option<String>,
        git_object_type: Option<String>,
        value: serde_json::Value,
    ) -> Self {
        Self {
            object_type: kind.history_type().to_string(),
            hash,
            git_object_type,
            summary: summarize_object_fields(kind, &value),
            raw_json_lines: pretty_json_lines(&value),
        }
    }

    fn unavailable(kind: GraphNodeKind, reason: impl Into<String>) -> Self {
        Self {
            object_type: kind.history_type().to_string(),
            hash: None,
            git_object_type: None,
            summary: vec![
                ("object_status".to_string(), "unavailable".to_string()),
                ("reason".to_string(), reason.into()),
            ],
            raw_json_lines: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct GraphObjectDetails {
    by_node: BTreeMap<(GraphNodeKind, String), GraphObjectDetail>,
}

impl GraphObjectDetails {
    fn get(&self, kind: GraphNodeKind, id: &str) -> Option<GraphObjectDetail> {
        self.by_node.get(&(kind, id.to_string())).cloned()
    }

    fn insert(&mut self, kind: GraphNodeKind, id: String, detail: GraphObjectDetail) {
        self.by_node.insert((kind, id), detail);
    }
}

impl ThreadGraph {
    fn from_projection(
        bundle: ThreadBundle,
        rows: ProjectionIndexRows,
        object_details: GraphObjectDetails,
    ) -> Self {
        let mut graph_rows = Vec::new();

        let selected_plan_ids = bundle
            .scheduler
            .selected_plan_ids
            .iter()
            .map(|plan| plan.plan_id.to_string())
            .collect::<BTreeSet<_>>();
        let head_plan_ids = bundle
            .scheduler
            .current_plan_heads
            .iter()
            .map(|plan| plan.plan_id.to_string())
            .collect::<BTreeSet<_>>();

        let plans_by_intent = group_values_by_key(rows.intent_plans.iter().map(|row| {
            (
                row.intent_id.clone(),
                TimedValue {
                    value: row.plan_id.clone(),
                    sort: row.created_at,
                },
            )
        }));
        let tasks_by_intent = group_values_by_key(rows.intent_tasks.iter().map(|row| {
            (
                row.intent_id.clone(),
                TimedValue {
                    value: row.task_id.clone(),
                    sort: row.created_at,
                },
            )
        }));
        let tasks_by_plan = group_values_by_key(rows.plan_tasks.iter().map(|row| {
            (
                row.plan_id.clone(),
                TimedValue {
                    value: row.task_id.clone(),
                    sort: row.created_at,
                },
            )
        }));
        let runs_by_task = group_values_by_key(rows.task_runs.iter().map(|row| {
            (
                row.task_id.clone(),
                TimedValue {
                    value: row.run_id.clone(),
                    sort: row.created_at,
                },
            )
        }));
        let patchsets_by_run = group_values_by_key(rows.run_patchsets.iter().map(|row| {
            (
                row.run_id.clone(),
                TimedValue {
                    value: row.patchset_id.clone(),
                    sort: row.sequence,
                },
            )
        }));
        let latest_run_events = rows
            .run_events
            .iter()
            .filter(|row| row.is_latest)
            .map(|row| (row.run_id.clone(), row.event_kind.clone()))
            .collect::<BTreeMap<_, _>>();
        let latest_patchsets = rows
            .run_patchsets
            .iter()
            .filter(|row| row.is_latest)
            .map(|row| row.patchset_id.clone())
            .collect::<BTreeSet<_>>();
        let latest_runs = rows
            .task_runs
            .iter()
            .filter(|row| row.is_latest)
            .map(|row| row.run_id.clone())
            .collect::<BTreeSet<_>>();

        let mut intents = bundle.thread.intents.clone();
        intents.sort_by_key(|intent| intent.ordinal);
        for intent in intents {
            let intent_id = intent.intent_id.to_string();
            let mut tags = vec![format!("{:?}", intent.link_reason)];
            if intent.is_head {
                tags.push("head".to_string());
            }
            if bundle.thread.current_intent_id == Some(intent.intent_id) {
                tags.push("current".to_string());
            }
            if bundle.thread.latest_intent_id == Some(intent.intent_id) {
                tags.push("latest".to_string());
            }

            graph_rows.push(GraphLine {
                depth: 0,
                kind: GraphNodeKind::Intent,
                id: intent_id.clone(),
                label: format!("#{} {}", intent.ordinal, short_id(&intent_id)),
                tags,
                detail: vec![
                    ("intent_id".to_string(), intent_id.clone()),
                    ("ordinal".to_string(), intent.ordinal.to_string()),
                    (
                        "link_reason".to_string(),
                        format!("{:?}", intent.link_reason),
                    ),
                    ("is_head".to_string(), intent.is_head.to_string()),
                    ("linked_at".to_string(), format_timestamp(intent.linked_at)),
                ],
                object: object_details.get(GraphNodeKind::Intent, &intent_id),
            });

            let mut displayed_tasks = BTreeSet::new();
            for plan_id in plans_by_intent.get(&intent_id).cloned().unwrap_or_default() {
                let mut plan_tags = Vec::new();
                if selected_plan_ids.contains(&plan_id) {
                    plan_tags.push("selected".to_string());
                }
                if head_plan_ids.contains(&plan_id) {
                    plan_tags.push("head".to_string());
                }

                graph_rows.push(GraphLine {
                    depth: 1,
                    kind: GraphNodeKind::Plan,
                    id: plan_id.clone(),
                    label: short_id(&plan_id),
                    tags: plan_tags,
                    detail: vec![
                        ("plan_id".to_string(), plan_id.clone()),
                        (
                            "selected".to_string(),
                            selected_plan_ids.contains(&plan_id).to_string(),
                        ),
                        (
                            "plan_head".to_string(),
                            head_plan_ids.contains(&plan_id).to_string(),
                        ),
                    ],
                    object: object_details.get(GraphNodeKind::Plan, &plan_id),
                });

                for task_id in tasks_by_plan.get(&plan_id).cloned().unwrap_or_default() {
                    displayed_tasks.insert(task_id.clone());
                    push_task_subgraph(
                        &mut graph_rows,
                        &task_id,
                        2,
                        &runs_by_task,
                        &patchsets_by_run,
                        &latest_runs,
                        &latest_run_events,
                        &latest_patchsets,
                        bundle.scheduler.active_task_id,
                        bundle.scheduler.active_run_id,
                        &object_details,
                    );
                }
            }

            for task_id in tasks_by_intent.get(&intent_id).cloned().unwrap_or_default() {
                if displayed_tasks.insert(task_id.clone()) {
                    push_task_subgraph(
                        &mut graph_rows,
                        &task_id,
                        1,
                        &runs_by_task,
                        &patchsets_by_run,
                        &latest_runs,
                        &latest_run_events,
                        &latest_patchsets,
                        bundle.scheduler.active_task_id,
                        bundle.scheduler.active_run_id,
                        &object_details,
                    );
                }
            }
        }

        ThreadGraph {
            thread_id: bundle.thread.thread_id,
            title: bundle.thread.title,
            freshness: format!("{:?}", bundle.freshness),
            thread_version: bundle.thread.version,
            scheduler_version: bundle.scheduler.version,
            updated_at: bundle.thread.updated_at.max(bundle.scheduler.updated_at),
            selected_plan_id: bundle.scheduler.selected_plan_id,
            active_task_id: bundle.scheduler.active_task_id,
            active_run_id: bundle.scheduler.active_run_id,
            lines: graph_rows,
        }
    }
}

async fn load_graph_object_details<S>(
    history: &HistoryManager,
    storage: &S,
    bundle: &ThreadBundle,
    rows: &ProjectionIndexRows,
) -> GraphObjectDetails
where
    S: Storage + ?Sized,
{
    let mut details = GraphObjectDetails::default();
    for (kind, id) in graph_object_refs(bundle, rows) {
        let detail = load_graph_object_detail(history, storage, kind, &id).await;
        details.insert(kind, id, detail);
    }
    details
}

async fn load_graph_object_detail<S>(
    history: &HistoryManager,
    storage: &S,
    kind: GraphNodeKind,
    object_id: &str,
) -> GraphObjectDetail
where
    S: Storage + ?Sized,
{
    let hash = match history
        .get_object_hash(kind.history_type(), object_id)
        .await
    {
        Ok(Some(hash)) => hash,
        Ok(None) => {
            return GraphObjectDetail::unavailable(
                kind,
                format!("{} object was not found in AI history", kind.history_type()),
            );
        }
        Err(error) => {
            return GraphObjectDetail::unavailable(
                kind,
                format!("failed to look up object in AI history: {error:#}"),
            );
        }
    };

    let (data, git_object_type) = match storage.get(&hash).await {
        Ok(found) => found,
        Err(error) => {
            return GraphObjectDetail::unavailable(
                kind,
                format!("failed to read object blob {hash}: {error}"),
            );
        }
    };

    let value = serde_json::from_slice::<serde_json::Value>(&data)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&data).to_string()));
    GraphObjectDetail::from_json(
        kind,
        Some(hash.to_string()),
        Some(format!("{git_object_type:?}")),
        value,
    )
}

fn graph_object_refs(
    bundle: &ThreadBundle,
    rows: &ProjectionIndexRows,
) -> BTreeSet<(GraphNodeKind, String)> {
    let mut refs = BTreeSet::new();

    for intent in &bundle.thread.intents {
        refs.insert((GraphNodeKind::Intent, intent.intent_id.to_string()));
    }
    if let Some(intent_id) = bundle.thread.current_intent_id {
        refs.insert((GraphNodeKind::Intent, intent_id.to_string()));
    }
    if let Some(intent_id) = bundle.thread.latest_intent_id {
        refs.insert((GraphNodeKind::Intent, intent_id.to_string()));
    }

    for plan in &bundle.scheduler.selected_plan_ids {
        refs.insert((GraphNodeKind::Plan, plan.plan_id.to_string()));
    }
    for plan in &bundle.scheduler.current_plan_heads {
        refs.insert((GraphNodeKind::Plan, plan.plan_id.to_string()));
    }
    if let Some(plan_id) = bundle.scheduler.selected_plan_id {
        refs.insert((GraphNodeKind::Plan, plan_id.to_string()));
    }
    if let Some(task_id) = bundle.scheduler.active_task_id {
        refs.insert((GraphNodeKind::Task, task_id.to_string()));
    }
    if let Some(run_id) = bundle.scheduler.active_run_id {
        refs.insert((GraphNodeKind::Run, run_id.to_string()));
    }

    for row in &rows.intent_plans {
        refs.insert((GraphNodeKind::Plan, row.plan_id.clone()));
    }
    for row in &rows.intent_tasks {
        refs.insert((GraphNodeKind::Task, row.task_id.clone()));
    }
    for row in &rows.plan_tasks {
        refs.insert((GraphNodeKind::Task, row.task_id.clone()));
    }
    for row in &rows.task_runs {
        refs.insert((GraphNodeKind::Run, row.run_id.clone()));
    }
    for row in &rows.run_patchsets {
        refs.insert((GraphNodeKind::Patchset, row.patchset_id.clone()));
    }

    refs
}

#[derive(Debug, Clone)]
struct TimedValue {
    value: String,
    sort: i64,
}

fn group_values_by_key(
    values: impl Iterator<Item = (String, TimedValue)>,
) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<TimedValue>>::new();
    for (key, value) in values {
        grouped.entry(key).or_default().push(value);
    }

    grouped
        .into_iter()
        .map(|(key, mut values)| {
            values.sort_by(|left, right| {
                left.sort
                    .cmp(&right.sort)
                    .then_with(|| left.value.cmp(&right.value))
            });
            values.dedup_by(|left, right| left.value == right.value);
            (key, values.into_iter().map(|value| value.value).collect())
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn push_task_subgraph(
    graph_rows: &mut Vec<GraphLine>,
    task_id: &str,
    depth: usize,
    runs_by_task: &BTreeMap<String, Vec<String>>,
    patchsets_by_run: &BTreeMap<String, Vec<String>>,
    latest_runs: &BTreeSet<String>,
    latest_run_events: &BTreeMap<String, String>,
    latest_patchsets: &BTreeSet<String>,
    active_task_id: Option<Uuid>,
    active_run_id: Option<Uuid>,
    object_details: &GraphObjectDetails,
) {
    let active_task = active_task_id
        .map(|id| id.to_string())
        .is_some_and(|id| id == task_id);
    let mut task_tags = Vec::new();
    if active_task {
        task_tags.push("active".to_string());
    }

    graph_rows.push(GraphLine {
        depth,
        kind: GraphNodeKind::Task,
        id: task_id.to_string(),
        label: short_id(task_id),
        tags: task_tags,
        detail: vec![
            ("task_id".to_string(), task_id.to_string()),
            ("active".to_string(), active_task.to_string()),
        ],
        object: object_details.get(GraphNodeKind::Task, task_id),
    });

    for run_id in runs_by_task.get(task_id).cloned().unwrap_or_default() {
        let active_run = active_run_id
            .map(|id| id.to_string())
            .is_some_and(|id| id == run_id);
        let mut run_tags = Vec::new();
        if latest_runs.contains(&run_id) {
            run_tags.push("latest".to_string());
        }
        if active_run {
            run_tags.push("active".to_string());
        }
        if let Some(event_kind) = latest_run_events.get(&run_id) {
            run_tags.push(event_kind.clone());
        }

        graph_rows.push(GraphLine {
            depth: depth + 1,
            kind: GraphNodeKind::Run,
            id: run_id.clone(),
            label: short_id(&run_id),
            tags: run_tags,
            detail: vec![
                ("run_id".to_string(), run_id.clone()),
                ("task_id".to_string(), task_id.to_string()),
                (
                    "latest_event".to_string(),
                    latest_run_events
                        .get(&run_id)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
                ("active".to_string(), active_run.to_string()),
            ],
            object: object_details.get(GraphNodeKind::Run, &run_id),
        });

        for patchset_id in patchsets_by_run.get(&run_id).cloned().unwrap_or_default() {
            let mut patchset_tags = Vec::new();
            if latest_patchsets.contains(&patchset_id) {
                patchset_tags.push("latest".to_string());
            }
            graph_rows.push(GraphLine {
                depth: depth + 2,
                kind: GraphNodeKind::Patchset,
                id: patchset_id.clone(),
                label: short_id(&patchset_id),
                tags: patchset_tags,
                detail: vec![
                    ("patchset_id".to_string(), patchset_id.clone()),
                    ("run_id".to_string(), run_id.clone()),
                ],
                object: object_details.get(GraphNodeKind::Patchset, &patchset_id),
            });
        }
    }
}

fn summarize_object_fields(
    kind: GraphNodeKind,
    value: &serde_json::Value,
) -> Vec<(String, String)> {
    let keys = match kind {
        GraphNodeKind::Intent => [
            "object_id",
            "created_at",
            "created_by",
            "prompt",
            "parents",
            "spec",
            "analysis_context_frames",
        ]
        .as_slice(),
        GraphNodeKind::Plan => [
            "object_id",
            "created_at",
            "created_by",
            "intent",
            "parents",
            "context_frames",
            "steps",
        ]
        .as_slice(),
        GraphNodeKind::Task => [
            "object_id",
            "created_at",
            "created_by",
            "title",
            "description",
            "goal",
            "constraints",
            "acceptance_criteria",
            "requester",
            "parent",
            "intent",
            "origin_step_id",
            "dependencies",
        ]
        .as_slice(),
        GraphNodeKind::Run => [
            "object_id",
            "created_at",
            "created_by",
            "task",
            "plan",
            "commit",
            "snapshot",
            "environment",
        ]
        .as_slice(),
        GraphNodeKind::Patchset => [
            "object_id",
            "created_at",
            "created_by",
            "run",
            "sequence",
            "commit",
            "format",
            "artifact",
            "touched",
            "rationale",
        ]
        .as_slice(),
    };

    let Some(object) = value.as_object() else {
        return vec![("value".to_string(), summarize_json_value(value))];
    };

    keys.iter()
        .filter_map(|key| {
            object
                .get(*key)
                .map(|value| ((*key).to_string(), summarize_json_value(value)))
        })
        .collect()
}

fn summarize_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => truncate_chars(value, MAX_OBJECT_DETAIL_LINE_CHARS),
        serde_json::Value::Array(values) => {
            if values.is_empty() {
                "[]".to_string()
            } else {
                format!("array[{}]", values.len())
            }
        }
        serde_json::Value::Object(values) => {
            if values.is_empty() {
                "{}".to_string()
            } else {
                format!("object{{{} keys}}", values.len())
            }
        }
    }
}

fn pretty_json_lines(value: &serde_json::Value) -> Vec<String> {
    let rendered = serde_json::to_string_pretty(value)
        .unwrap_or_else(|error| format!("failed to render object JSON: {error}"));
    let mut lines = Vec::new();
    for (index, line) in rendered.lines().enumerate() {
        if index >= MAX_OBJECT_DETAIL_LINES {
            lines.push(format!(
                "... truncated after {MAX_OBJECT_DETAIL_LINES} object lines"
            ));
            break;
        }
        lines.push(truncate_chars(line, MAX_OBJECT_DETAIL_LINE_CHARS));
    }
    lines
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn run_graph_tui(graph: ThreadGraph) -> std::io::Result<()> {
    let terminal = tui_init()?;
    let _guard = scopeguard::guard((), |_| {
        let _ = tui_restore();
    });
    let mut tui = Tui::new(terminal);
    tui.enter_alt_screen()?;
    let mut app = GraphTuiApp::new(graph);

    loop {
        tui.draw(|frame| render_graph(frame, &mut app))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == event::KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Up => app.select_previous(),
                KeyCode::Down => app.select_next(),
                KeyCode::Home => app.select_first(),
                KeyCode::End => app.select_last(),
                KeyCode::PageUp => app.scroll_details_page_up(),
                KeyCode::PageDown => app.scroll_details_page_down(),
                KeyCode::Char('[') => app.scroll_details_up(),
                KeyCode::Char(']') => app.scroll_details_down(),
                _ => {}
            }
        }
    }

    tui.leave_alt_screen()?;
    Ok(())
}

#[derive(Debug, Clone)]
struct GraphTuiApp {
    graph: ThreadGraph,
    selected: usize,
    scroll: usize,
    page_size: usize,
    detail_scroll: usize,
    detail_page_size: usize,
}

impl GraphTuiApp {
    fn new(graph: ThreadGraph) -> Self {
        Self {
            graph,
            selected: 0,
            scroll: 0,
            page_size: 1,
            detail_scroll: 0,
            detail_page_size: 1,
        }
    }

    fn set_selected(&mut self, selected: usize) {
        if self.selected != selected {
            self.selected = selected;
            self.detail_scroll = 0;
        }
    }

    fn select_previous(&mut self) {
        self.set_selected(self.selected.saturating_sub(1));
    }

    fn select_next(&mut self) {
        if self.selected + 1 < self.graph.lines.len() {
            self.set_selected(self.selected + 1);
        }
    }

    fn select_first(&mut self) {
        self.set_selected(0);
    }

    fn select_last(&mut self) {
        self.set_selected(self.graph.lines.len().saturating_sub(1));
    }

    fn scroll_details_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(1);
    }

    fn scroll_details_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(1);
    }

    fn scroll_details_page_up(&mut self) {
        self.detail_scroll = self
            .detail_scroll
            .saturating_sub(self.detail_page_size.max(1));
    }

    fn scroll_details_page_down(&mut self) {
        self.detail_scroll = self
            .detail_scroll
            .saturating_add(self.detail_page_size.max(1));
    }

    fn keep_selection_visible(&mut self, height: usize) {
        self.page_size = height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        let bottom = self.scroll.saturating_add(height);
        if self.selected >= bottom {
            self.scroll = self.selected.saturating_sub(height.saturating_sub(1));
        }
        let max_scroll = self.graph.lines.len().saturating_sub(height);
        self.scroll = self.scroll.min(max_scroll);
    }

    fn keep_detail_scroll_bounded(&mut self, line_count: usize, height: usize) {
        self.detail_page_size = height.max(1);
        let max_scroll = line_count.saturating_sub(height.max(1));
        self.detail_scroll = self.detail_scroll.min(max_scroll);
    }
}

fn render_graph(frame: &mut Frame, app: &mut GraphTuiApp) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, vertical[0], &app.graph);

    let body = split_graph_body(vertical[1]);

    let graph_inner_height = body[0].height.saturating_sub(2) as usize;
    app.keep_selection_visible(graph_inner_height);
    render_graph_lines(frame, body[0], app);
    render_details(frame, body[1], app);
    render_footer(frame, vertical[2]);
}

fn split_graph_body(area: Rect) -> [Rect; 2] {
    let total_weight = GRAPH_PANE_WEIGHT + DETAILS_PANE_WEIGHT;
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(GRAPH_PANE_WEIGHT, total_weight),
            Constraint::Ratio(DETAILS_PANE_WEIGHT, total_weight),
        ])
        .split(area);

    [body[0], body[1]]
}

fn render_header(frame: &mut Frame, area: Rect, graph: &ThreadGraph) {
    let title = graph.title.as_deref().unwrap_or("Untitled thread");
    let lines = vec![
        Line::from(vec![
            Span::styled(
                "Thread Version Graph",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                short_id(&graph.thread_id.to_string()),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled(&graph.freshness, Style::default().fg(Color::Green)),
        ]),
        Line::raw(title.to_string()),
        Line::raw(format!(
            "thread v{} | scheduler v{} | updated {}",
            graph.thread_version,
            graph.scheduler_version,
            format_timestamp(graph.updated_at)
        )),
        Line::raw(format!(
            "selected plan: {} | active task: {} | active run: {}",
            graph
                .selected_plan_id
                .map(|id| short_id(&id.to_string()))
                .unwrap_or_else(|| "-".to_string()),
            graph
                .active_task_id
                .map(|id| short_id(&id.to_string()))
                .unwrap_or_else(|| "-".to_string()),
            graph
                .active_run_id
                .map(|id| short_id(&id.to_string()))
                .unwrap_or_else(|| "-".to_string()),
        )),
    ];
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn render_graph_lines(frame: &mut Frame, area: Rect, app: &GraphTuiApp) {
    let visible = area.height.saturating_sub(2) as usize;
    let end = app
        .scroll
        .saturating_add(visible)
        .min(app.graph.lines.len());
    let lines = if app.graph.lines.is_empty() {
        vec![Line::styled(
            "No version nodes are available for this thread.",
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        app.graph.lines[app.scroll..end]
            .iter()
            .enumerate()
            .map(|(offset, line)| render_graph_line(line, app.scroll + offset == app.selected))
            .collect::<Vec<_>>()
    };
    let block = Block::default().borders(Borders::ALL).title(" Graph ");
    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

fn render_graph_line(line: &GraphLine, selected: bool) -> Line<'static> {
    let selected_style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let mut spans = Vec::new();
    spans.push(Span::styled(
        if selected { "> " } else { "  " },
        selected_style,
    ));
    spans.push(Span::styled(indent_for_depth(line.depth), selected_style));
    spans.push(Span::styled(
        format!("[{}] ", line.kind.marker()),
        Style::default()
            .fg(line.kind.color())
            .add_modifier(Modifier::BOLD)
            .patch(selected_style),
    ));
    spans.push(Span::styled(line.label.clone(), selected_style));
    for tag in &line.tags {
        spans.push(Span::styled(
            format!(" <{tag}>"),
            Style::default().fg(Color::DarkGray).patch(selected_style),
        ));
    }
    Line::from(spans)
}

fn render_details(frame: &mut Frame, area: Rect, app: &mut GraphTuiApp) {
    let visible = area.height.saturating_sub(2) as usize;
    let content_width = area.width.saturating_sub(2) as usize;
    let lines = detail_lines_for_width(app.graph.lines.get(app.selected), content_width);
    app.keep_detail_scroll_bounded(lines.len(), visible);
    let scroll = app.detail_scroll as u16;
    let scrolled = app.detail_scroll > 0;
    let has_more = app.detail_scroll.saturating_add(visible) < lines.len();
    let title = match (scrolled, has_more) {
        (true, true) => " Details * ",
        (true, false) => " Details ^ ",
        (false, true) => " Details v ",
        (false, false) => " Details ",
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

fn detail_lines_for_width(
    selected: Option<&GraphLine>,
    content_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(line) = selected {
        lines.push(Line::from(vec![
            Span::styled(
                line.kind.label(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(short_id(&line.id), Style::default().fg(line.kind.color())),
        ]));
        lines.push(Line::raw(""));
        for (key, value) in &line.detail {
            push_detail_kv(&mut lines, key, value, content_width);
        }
        if !line.tags.is_empty() {
            lines.push(Line::raw(""));
            push_detail_kv(&mut lines, "tags", &line.tags.join(", "), content_width);
        }
        if let Some(object) = &line.object {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Object",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push_detail_kv(
                &mut lines,
                "object_type",
                &object.object_type,
                content_width,
            );
            if let Some(hash) = &object.hash {
                push_detail_kv(&mut lines, "object_hash", hash, content_width);
            }
            if let Some(git_object_type) = &object.git_object_type {
                push_detail_kv(
                    &mut lines,
                    "git_object_type",
                    git_object_type,
                    content_width,
                );
            }
            for (key, value) in &object.summary {
                push_detail_kv(&mut lines, key, value, content_width);
            }
            if !object.raw_json_lines.is_empty() {
                lines.push(Line::raw(""));
                lines.push(Line::styled("object_json:", detail_label_style()));
                for raw_line in &object.raw_json_lines {
                    push_detail_wrapped_line(&mut lines, raw_line, content_width);
                }
            }
        }
    } else {
        lines.push(Line::raw("No node selected."));
    }
    lines
}

fn detail_label_style() -> Style {
    Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD)
}

fn detail_value_style() -> Style {
    Style::default().fg(Color::White)
}

fn push_detail_kv(lines: &mut Vec<Line<'static>>, key: &str, value: &str, content_width: usize) {
    let key_text = format!("{key}: ");
    let key_width = display_width(&key_text);
    if content_width != usize::MAX
        && key_width.saturating_add(MIN_DETAIL_VALUE_WIDTH) > content_width
    {
        lines.push(Line::styled(
            key_text.trim_end().to_string(),
            detail_label_style(),
        ));
        let value_width = content_width.saturating_sub(2).max(1);
        for chunk in wrap_display_width(value, value_width) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(chunk, detail_value_style()),
            ]));
        }
        return;
    }

    let value_width = if content_width == usize::MAX {
        usize::MAX
    } else {
        content_width
            .saturating_sub(key_width)
            .max(MIN_DETAIL_VALUE_WIDTH)
    };
    let chunks = wrap_display_width(value, value_width);
    let mut iter = chunks.into_iter();
    let first = iter.next().unwrap_or_default();
    lines.push(Line::from(vec![
        Span::styled(key_text.clone(), detail_label_style()),
        Span::styled(first, detail_value_style()),
    ]));

    let indent = if content_width == usize::MAX {
        " ".repeat(key_width)
    } else {
        " ".repeat(key_width.min(content_width.saturating_sub(1)))
    };
    for chunk in iter {
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(chunk, detail_value_style()),
        ]));
    }
}

fn push_detail_wrapped_line(lines: &mut Vec<Line<'static>>, value: &str, content_width: usize) {
    for chunk in wrap_display_width(value, content_width.max(1)) {
        lines.push(Line::styled(chunk, detail_value_style()));
    }
}

fn wrap_display_width(value: &str, max_width: usize) -> Vec<String> {
    if value.is_empty() {
        return vec![String::new()];
    }
    if max_width == usize::MAX {
        return value.lines().map(ToString::to_string).collect();
    }

    let width = max_width.max(1);
    let mut lines = Vec::new();
    for source_line in value.lines() {
        let mut current = String::new();
        let mut current_width = 0usize;
        for ch in source_line.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if current_width > 0 && current_width.saturating_add(ch_width) > width {
                lines.push(current.trim_end().to_string());
                current = String::new();
                current_width = 0;
            }
            if current_width == 0 && ch.is_whitespace() {
                continue;
            }
            current.push(ch);
            current_width = current_width.saturating_add(ch_width);
        }
        lines.push(current.trim_end().to_string());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn display_width(value: &str) -> usize {
    value
        .chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn render_footer(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(
            "Up/Down select  Home/End bounds  PageUp/PageDown details  [/] line  q/Esc quit",
        )
        .style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn indent_for_depth(depth: usize) -> String {
    if depth == 0 {
        return String::new();
    }

    format!("{}|-- ", "  ".repeat(depth.saturating_sub(1)))
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use git_internal::internal::object::types::ActorRef;

    use super::*;
    use crate::internal::ai::{
        projection::{
            PlanHeadRef, SchedulerState, ThreadIntentLinkReason, ThreadIntentRef,
            ThreadParticipant, ThreadParticipantRole, ThreadProjection,
        },
        runtime::contracts::ProjectionFreshness,
    };

    fn id(value: &str) -> Uuid {
        Uuid::parse_str(value).expect("test UUID should be valid")
    }

    fn ts(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .expect("test timestamp should be valid")
    }

    fn sample_bundle() -> ThreadBundle {
        let thread_id = id("11111111-1111-4111-8111-111111111111");
        let intent_id = id("22222222-2222-4222-8222-222222222222");
        let plan_id = id("33333333-3333-4333-8333-333333333333");
        let task_id = id("44444444-4444-4444-8444-444444444444");
        let run_id = id("55555555-5555-4555-8555-555555555555");
        let owner = ActorRef::human("graph-test").expect("actor");

        ThreadBundle {
            thread: ThreadProjection {
                thread_id,
                title: Some("Graph test".to_string()),
                owner: owner.clone(),
                participants: vec![ThreadParticipant {
                    actor: owner,
                    role: ThreadParticipantRole::Owner,
                    joined_at: ts(1),
                }],
                current_intent_id: Some(intent_id),
                latest_intent_id: Some(intent_id),
                intents: vec![ThreadIntentRef {
                    intent_id,
                    ordinal: 0,
                    is_head: true,
                    linked_at: ts(2),
                    link_reason: ThreadIntentLinkReason::Seed,
                }],
                metadata: None,
                archived: false,
                created_at: ts(1),
                updated_at: ts(10),
                version: 2,
            },
            scheduler: SchedulerState {
                thread_id,
                selected_plan_id: Some(plan_id),
                selected_plan_ids: vec![PlanHeadRef {
                    plan_id,
                    ordinal: 0,
                }],
                current_plan_heads: vec![PlanHeadRef {
                    plan_id,
                    ordinal: 0,
                }],
                active_task_id: Some(task_id),
                active_run_id: Some(run_id),
                live_context_window: Vec::new(),
                metadata: None,
                updated_at: ts(11),
                version: 3,
            },
            freshness: ProjectionFreshness::Fresh,
        }
    }

    #[test]
    fn graph_model_orders_thread_versions_from_projection_indexes() {
        let bundle = sample_bundle();
        let rows = ProjectionIndexRows {
            intent_plans: vec![ai_index_intent_plan::Model {
                intent_id: "22222222-2222-4222-8222-222222222222".to_string(),
                plan_id: "33333333-3333-4333-8333-333333333333".to_string(),
                created_at: 3,
            }],
            intent_tasks: vec![ai_index_intent_task::Model {
                intent_id: "22222222-2222-4222-8222-222222222222".to_string(),
                task_id: "44444444-4444-4444-8444-444444444444".to_string(),
                parent_task_id: None,
                origin_step_id: None,
                created_at: 4,
            }],
            plan_tasks: vec![ai_index_plan_step_task::Model {
                plan_id: "33333333-3333-4333-8333-333333333333".to_string(),
                task_id: "44444444-4444-4444-8444-444444444444".to_string(),
                step_id: "66666666-6666-4666-8666-666666666666".to_string(),
                created_at: 5,
            }],
            task_runs: vec![ai_index_task_run::Model {
                task_id: "44444444-4444-4444-8444-444444444444".to_string(),
                run_id: "55555555-5555-4555-8555-555555555555".to_string(),
                is_latest: true,
                created_at: 6,
            }],
            run_events: vec![ai_index_run_event::Model {
                run_id: "55555555-5555-4555-8555-555555555555".to_string(),
                event_id: "77777777-7777-4777-8777-777777777777".to_string(),
                event_kind: "completed".to_string(),
                is_latest: true,
                created_at: 7,
            }],
            run_patchsets: vec![ai_index_run_patchset::Model {
                run_id: "55555555-5555-4555-8555-555555555555".to_string(),
                patchset_id: "88888888-8888-4888-8888-888888888888".to_string(),
                sequence: 1,
                is_latest: true,
                created_at: 8,
            }],
        };

        let graph = ThreadGraph::from_projection(bundle, rows, GraphObjectDetails::default());
        let kinds = graph.lines.iter().map(|line| line.kind).collect::<Vec<_>>();

        assert_eq!(
            kinds,
            vec![
                GraphNodeKind::Intent,
                GraphNodeKind::Plan,
                GraphNodeKind::Task,
                GraphNodeKind::Run,
                GraphNodeKind::Patchset,
            ]
        );
        assert!(graph.lines[1].tags.contains(&"selected".to_string()));
        assert!(graph.lines[2].tags.contains(&"active".to_string()));
        assert!(graph.lines[3].tags.contains(&"completed".to_string()));
    }

    #[test]
    fn graph_line_uses_ascii_tree_prefixes() {
        let line = GraphLine {
            depth: 2,
            kind: GraphNodeKind::Run,
            id: "55555555-5555-4555-8555-555555555555".to_string(),
            label: "55555555".to_string(),
            tags: vec!["latest".to_string()],
            detail: Vec::new(),
            object: None,
        };

        let rendered = render_graph_line(&line, false);
        let text = rendered
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("  |-- [R] 55555555 <latest>"));
    }

    #[test]
    fn graph_body_layout_prioritizes_details_with_one_to_two_ratio() {
        let [graph_area, details_area] = split_graph_body(Rect::new(0, 0, 120, 20));

        assert_eq!(graph_area.width, 40);
        assert_eq!(details_area.width, 80);
        assert_eq!(details_area.x, graph_area.x + graph_area.width);
    }

    #[test]
    fn graph_details_include_persisted_object_content() {
        let detail = GraphObjectDetail::from_json(
            GraphNodeKind::Task,
            Some("abc123".to_string()),
            Some("Blob".to_string()),
            serde_json::json!({
                "object_id": "44444444-4444-4444-8444-444444444444",
                "object_type": "task",
                "title": "Render graph object details",
                "description": "Show the stored task object, not just projection links",
                "constraints": ["keep graph responsive"],
                "acceptance_criteria": ["details panel includes object_json"]
            }),
        );
        let line = GraphLine {
            depth: 1,
            kind: GraphNodeKind::Task,
            id: "44444444-4444-4444-8444-444444444444".to_string(),
            label: "44444444".to_string(),
            tags: Vec::new(),
            detail: vec![(
                "task_id".to_string(),
                "44444444-4444-4444-8444-444444444444".to_string(),
            )],
            object: Some(detail),
        };

        let rendered = detail_lines_for_width(Some(&line), usize::MAX)
            .into_iter()
            .flat_map(|line| line.spans.into_iter())
            .map(|span| span.content.into_owned())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("object_hash: "));
        assert!(rendered.contains("abc123"));
        assert!(rendered.contains("title: "));
        assert!(rendered.contains("Render graph object details"));
        assert!(rendered.contains("object_json:"));
        assert!(rendered.contains("Show the stored task object"));
    }

    #[test]
    fn graph_detail_lines_wrap_long_and_wide_text_to_panel_width() {
        let line = GraphLine {
            depth: 1,
            kind: GraphNodeKind::Task,
            id: "44444444-4444-4444-8444-444444444444".to_string(),
            label: "44444444".to_string(),
            tags: Vec::new(),
            detail: vec![
                (
                    "object_hash".to_string(),
                    "5ed4uc979f5d0b64126a4c1209b5d5d14824297".to_string(),
                ),
                (
                    "title".to_string(),
                    "初始化 Rust 项目并实现 CLI 子命令".to_string(),
                ),
                ("acceptance_criteria".to_string(), "array[3]".to_string()),
            ],
            object: None,
        };

        let texts = detail_lines_for_width(Some(&line), 32)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(texts.iter().any(|line| line.contains("object_hash:")));
        assert!(texts.iter().any(|line| line.contains("初始化 Rust 项目")));
        assert!(texts.iter().any(|line| line == "acceptance_criteria:"));
        for text in texts {
            assert!(
                display_width(&text) <= 32,
                "detail line exceeded panel width: {text:?}"
            );
        }
    }

    #[test]
    fn page_up_down_scroll_details_without_changing_selection() {
        let graph = ThreadGraph {
            thread_id: id("11111111-1111-4111-8111-111111111111"),
            title: None,
            freshness: "Fresh".to_string(),
            thread_version: 1,
            scheduler_version: 1,
            updated_at: ts(1),
            selected_plan_id: None,
            active_task_id: None,
            active_run_id: None,
            lines: vec![
                GraphLine {
                    depth: 0,
                    kind: GraphNodeKind::Intent,
                    id: "22222222-2222-4222-8222-222222222222".to_string(),
                    label: "22222222".to_string(),
                    tags: Vec::new(),
                    detail: Vec::new(),
                    object: None,
                },
                GraphLine {
                    depth: 1,
                    kind: GraphNodeKind::Task,
                    id: "44444444-4444-4444-8444-444444444444".to_string(),
                    label: "44444444".to_string(),
                    tags: Vec::new(),
                    detail: Vec::new(),
                    object: None,
                },
            ],
        };
        let mut app = GraphTuiApp::new(graph);
        app.select_next();
        app.detail_page_size = 5;

        app.scroll_details_page_down();
        assert_eq!(app.selected, 1);
        assert_eq!(app.detail_scroll, 5);

        app.scroll_details_page_up();
        assert_eq!(app.selected, 1);
        assert_eq!(app.detail_scroll, 0);
    }
}
