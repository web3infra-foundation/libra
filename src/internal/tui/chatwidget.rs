//! Main chat widget for displaying conversation history.
//!
//! Renders the scrollable chat area with history cells.

use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use uuid::Uuid;

use super::{
    bottom_pane::BottomPane,
    history_cell::{AssistantHistoryCell, HistoryCell, ThinkingHistoryCell, ToolCallHistoryCell},
    theme,
};
use crate::internal::ai::orchestrator::types::{
    ExecutionPlanSpec, TaskKind, TaskNodeStatus, TaskRuntimeEvent, TaskRuntimeNoteLevel,
    TaskRuntimePhase, TaskWorkspaceBackend,
};

#[derive(Debug, Clone)]
struct DagPanelNode {
    task_id: Uuid,
    kind: TaskKind,
    title: String,
    dependency_count: usize,
    dependencies: Vec<Uuid>,
    depth: usize,
    /// Column within the depth layer (0..lanes_at_depth).
    lane: usize,
    ordinal: usize,
    status: TaskNodeStatus,
}

#[derive(Debug, Clone, Default)]
struct DagPanelState {
    intent_spec_id: String,
    revision: u32,
    parent_revision: Option<u32>,
    replan_reason: Option<String>,
    max_parallel: u8,
    checkpoint_count: usize,
    completed: usize,
    total: usize,
    nodes: Vec<DagPanelNode>,
    validation_status: DagStageStatus,
    release_status: DagStageStatus,
}

impl DagPanelState {
    fn new(plan: ExecutionPlanSpec) -> Self {
        let groups = plan.scheduled_groups();
        let mut id_to_depth = std::collections::HashMap::new();
        for (depth, group) in groups.iter().enumerate() {
            for id in group {
                id_to_depth.insert(*id, depth);
            }
        }

        // Stable lane assignment: tasks at the same depth keep the order they
        // appear in `plan.tasks`, so re-renders don't shuffle columns.
        let mut lane_cursor: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        let nodes = plan
            .tasks
            .iter()
            .enumerate()
            .map(|(idx, task)| {
                let depth = id_to_depth.get(&task.id()).copied().unwrap_or_default();
                let lane = lane_cursor.entry(depth).or_default();
                let assigned_lane = *lane;
                *lane += 1;
                DagPanelNode {
                    task_id: task.id(),
                    kind: task.kind.clone(),
                    title: task.title().to_string(),
                    dependency_count: task.dependencies().len(),
                    dependencies: task.dependencies().to_vec(),
                    depth,
                    lane: assigned_lane,
                    ordinal: idx + 1,
                    status: TaskNodeStatus::Pending,
                }
            })
            .collect::<Vec<_>>();

        Self {
            intent_spec_id: plan.intent_spec_id.clone(),
            revision: plan.revision,
            parent_revision: plan.parent_revision,
            replan_reason: plan
                .replan_reason
                .as_ref()
                .map(|reason| reason.trim().to_string())
                .filter(|reason| !reason.is_empty()),
            max_parallel: plan.max_parallel,
            checkpoint_count: plan.checkpoints.len(),
            total: plan.tasks.len(),
            completed: 0,
            nodes,
            validation_status: DagStageStatus::Pending,
            release_status: DagStageStatus::Pending,
        }
    }

    fn update_task_status(&mut self, task_id: Uuid, status: TaskNodeStatus) {
        if let Some(node) = self.nodes.iter_mut().find(|node| node.task_id == task_id) {
            node.status = status;
        }
        self.completed = self.active_or_terminal_node_count().min(self.total);
    }

    fn update_progress(&mut self, completed: usize, total: usize) {
        self.total = total.max(self.total).max(self.nodes.len());
        self.completed = completed
            .min(self.total)
            .max(self.active_or_terminal_node_count().min(self.total))
            .max(self.terminal_node_count().min(self.total));
    }

    fn update_validation_status(&mut self, passed: bool) {
        self.validation_status = DagStageStatus::from_passed(passed);
    }

    fn update_release_status(&mut self, passed: bool) {
        self.release_status = DagStageStatus::from_passed(passed);
    }

    fn terminal_node_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.status,
                    TaskNodeStatus::Completed | TaskNodeStatus::Failed | TaskNodeStatus::Skipped
                )
            })
            .count()
    }

    fn active_or_terminal_node_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.status != TaskNodeStatus::Pending)
            .count()
    }

    fn lane_count(&self) -> usize {
        let mut per_depth = std::collections::HashMap::<usize, usize>::new();
        for node in &self.nodes {
            *per_depth.entry(node.depth).or_default() += 1;
        }
        per_depth.into_values().max().unwrap_or(0)
    }

    fn depth_count(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.depth)
            .max()
            .map(|max| max + 1)
            .unwrap_or(0)
    }

    fn has_running(&self) -> bool {
        self.nodes
            .iter()
            .any(|node| node.status == TaskNodeStatus::Running)
    }

    fn failed_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.status == TaskNodeStatus::Failed)
            .count()
    }

    /// Width-bounded human-readable lines describing the current thread/plan
    /// context (intent identifier, revision lineage, replan reason, parallel
    /// budget, checkpoint count). Returns an empty vec when the available
    /// width is too small to render anything meaningful.
    fn context_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width < 18 {
            return Vec::new();
        }
        let mut lines: Vec<Line<'static>> = Vec::new();

        if !self.intent_spec_id.is_empty() {
            let intent = truncate_label(&self.intent_spec_id, width.saturating_sub(7) as usize);
            lines.push(Line::from(vec![
                Span::styled("intent ", theme::text::muted()),
                Span::styled(intent, theme::text::primary().add_modifier(Modifier::BOLD)),
            ]));
        }

        let lineage = match self.parent_revision {
            Some(parent) => format!("plan r{} ← r{}", self.revision, parent),
            None => format!("plan r{}", self.revision),
        };
        let parallel = format!("· parallel {}", self.max_parallel.max(1));
        let checkpoints = if self.checkpoint_count > 0 {
            format!(" · checkpoints {}", self.checkpoint_count)
        } else {
            String::new()
        };
        let line = format!("{lineage} {parallel}{checkpoints}");
        lines.push(Line::styled(
            truncate_label(&line, width as usize),
            theme::text::muted(),
        ));

        if let Some(reason) = self.replan_reason.as_ref() {
            let label = format!("replan: {reason}");
            lines.push(Line::styled(
                truncate_label(&label, width as usize),
                theme::status::warning().add_modifier(Modifier::DIM),
            ));
        }

        lines
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DagStageStatus {
    #[default]
    Pending,
    Complete,
    Failed,
}

impl DagStageStatus {
    fn from_passed(passed: bool) -> Self {
        if passed { Self::Complete } else { Self::Failed }
    }
}

fn animation_phase(step_ms: u128) -> usize {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    (millis / step_ms.max(1)) as usize
}

#[derive(Debug, Clone)]
struct TaskMuxNoteEntry {
    level: TaskRuntimeNoteLevel,
    text: String,
}

#[derive(Debug, Clone)]
enum TaskMuxTranscriptEntry {
    Note(TaskMuxNoteEntry),
    Thinking(ThinkingHistoryCell),
    Assistant(AssistantHistoryCell),
    Tool(ToolCallHistoryCell),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskMuxMode {
    Overview,
    Focused,
}

#[derive(Debug, Clone)]
struct TaskMuxTransition {
    from_mode: TaskMuxMode,
    to_mode: TaskMuxMode,
    from_focus: usize,
    to_focus: usize,
    started_at: Instant,
}

#[derive(Debug, Clone)]
struct TaskMuxTaskState {
    task_id: Uuid,
    kind: TaskKind,
    title: String,
    depth: usize,
    ordinal: usize,
    status: TaskNodeStatus,
    phase: TaskRuntimePhase,
    working_dir: Option<PathBuf>,
    workspace_backend: Option<TaskWorkspaceBackend>,
    main_working_dir: Option<PathBuf>,
    isolated: bool,
    transcript: Vec<TaskMuxTranscriptEntry>,
}

#[derive(Debug, Clone, Copy)]
struct TaskWindowRenderState {
    selected: bool,
    focused: bool,
    emphasis: f32,
}

#[derive(Debug, Clone)]
struct TaskMuxState {
    revision: u32,
    selected: usize,
    focused: usize,
    mode: TaskMuxMode,
    transition: Option<TaskMuxTransition>,
    tasks: Vec<TaskMuxTaskState>,
}

const TASK_MUX_MAX_ENTRIES: usize = 96;
const TASK_MUX_TRANSITION_DURATION: Duration = Duration::from_millis(220);
const DAG_PANEL_WIDTH_PERCENT: u16 = 35;

impl TaskMuxState {
    fn current_focus_index(&self) -> usize {
        self.transition
            .as_ref()
            .map(|transition| transition.to_focus)
            .unwrap_or_else(|| {
                if self.mode == TaskMuxMode::Focused {
                    self.focused
                } else {
                    self.selected
                }
            })
            .min(self.tasks.len().saturating_sub(1))
    }

    fn current_focus_task(&self) -> Option<&TaskMuxTaskState> {
        self.tasks.get(self.current_focus_index())
    }

    fn update_task_status(&mut self, task_id: Uuid, status: TaskNodeStatus) {
        if let Some(task) = self.tasks.iter_mut().find(|task| task.task_id == task_id) {
            task.status = status;
        }
    }

    fn apply_runtime_event(&mut self, task_id: Uuid, event: TaskRuntimeEvent) {
        let Some(task) = self.tasks.iter_mut().find(|task| task.task_id == task_id) else {
            return;
        };

        match event {
            TaskRuntimeEvent::Phase(phase) => task.phase = phase,
            TaskRuntimeEvent::WorkspaceReady {
                working_dir,
                isolated,
                backend,
                main_working_dir,
            } => {
                task.working_dir = Some(working_dir);
                task.isolated = isolated;
                task.workspace_backend = Some(backend);
                task.main_working_dir = main_working_dir;
            }
            TaskRuntimeEvent::Note { level, text } => {
                complete_streaming_task_thinking(&mut task.transcript);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return;
                }
                push_task_transcript_entry(
                    &mut task.transcript,
                    TaskMuxTranscriptEntry::Note(TaskMuxNoteEntry {
                        level,
                        text: trimmed.to_string(),
                    }),
                );
            }
            TaskRuntimeEvent::ThinkingDelta(delta) => {
                if delta.is_empty() {
                    return;
                }
                if let Some(TaskMuxTranscriptEntry::Thinking(cell)) = task.transcript.last_mut()
                    && cell.is_streaming
                {
                    cell.append(&delta);
                    return;
                }

                let mut cell = ThinkingHistoryCell::streaming();
                cell.append(&delta);
                push_task_transcript_entry(
                    &mut task.transcript,
                    TaskMuxTranscriptEntry::Thinking(cell),
                );
            }
            TaskRuntimeEvent::AssistantMessage(text) => {
                complete_streaming_task_thinking(&mut task.transcript);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return;
                }
                push_task_transcript_entry(
                    &mut task.transcript,
                    TaskMuxTranscriptEntry::Assistant(AssistantHistoryCell::new(
                        trimmed.to_string(),
                    )),
                );
            }
            TaskRuntimeEvent::ToolCallBegin {
                call_id,
                tool_name,
                arguments,
            } => {
                complete_streaming_task_thinking(&mut task.transcript);
                if let Some(TaskMuxTranscriptEntry::Tool(cell)) = task.transcript.last_mut()
                    && cell.can_merge(&tool_name)
                {
                    cell.append_call(call_id, tool_name, arguments);
                    return;
                }

                push_task_transcript_entry(
                    &mut task.transcript,
                    TaskMuxTranscriptEntry::Tool(ToolCallHistoryCell::new(
                        call_id, tool_name, arguments,
                    )),
                );
            }
            TaskRuntimeEvent::ToolCallEnd {
                call_id,
                tool_name: _tool_name,
                result,
            } => {
                complete_streaming_task_thinking(&mut task.transcript);
                let mut pending_result = Some(result);
                for idx in (0..task.transcript.len()).rev() {
                    let Some(TaskMuxTranscriptEntry::Tool(cell)) = task.transcript.get_mut(idx)
                    else {
                        continue;
                    };
                    if !cell.contains_call_id(&call_id) {
                        continue;
                    }
                    if let Some(result) = pending_result.take() {
                        cell.complete_call(&call_id, result);
                    }
                    break;
                }
            }
        }
    }

    fn interrupt_running_tool_calls(&mut self) {
        for task in &mut self.tasks {
            for entry in &mut task.transcript {
                if let TaskMuxTranscriptEntry::Tool(cell) = entry {
                    cell.interrupt_running();
                }
            }
        }
    }

    fn next(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let next = (self.current_focus_index() + 1) % self.tasks.len();
        self.set_selected(next);
    }

    fn prev(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let current = self.current_focus_index();
        let next = if current == 0 {
            self.tasks.len() - 1
        } else {
            current - 1
        };
        self.set_selected(next);
    }

    fn set_selected(&mut self, index: usize) {
        let index = index.min(self.tasks.len().saturating_sub(1));
        self.selected = index;
        if self.mode == TaskMuxMode::Focused {
            self.start_transition(TaskMuxMode::Focused, index);
        }
    }

    fn focus_selected(&mut self) {
        self.start_transition(TaskMuxMode::Focused, self.selected);
    }

    fn toggle_mode(&mut self) {
        let target = if self.mode == TaskMuxMode::Focused {
            TaskMuxMode::Overview
        } else {
            TaskMuxMode::Focused
        };
        let target_focus = self.selected.min(self.tasks.len().saturating_sub(1));
        self.start_transition(target, target_focus);
    }

    fn show_overview(&mut self) {
        let target_focus = self.selected.min(self.tasks.len().saturating_sub(1));
        self.start_transition(TaskMuxMode::Overview, target_focus);
    }

    fn focus_index(&mut self, index: usize) -> bool {
        if index >= self.tasks.len() {
            return false;
        }
        self.selected = index;
        self.start_transition(TaskMuxMode::Focused, index);
        true
    }

    fn start_transition(&mut self, to_mode: TaskMuxMode, to_focus: usize) {
        if self.tasks.is_empty() {
            return;
        }
        let to_focus = to_focus.min(self.tasks.len().saturating_sub(1));
        let current_focus = self.current_focus_index();
        if self.transition.is_none() && self.mode == to_mode && current_focus == to_focus {
            return;
        }
        self.transition = Some(TaskMuxTransition {
            from_mode: self.mode,
            to_mode,
            from_focus: current_focus,
            to_focus,
            started_at: Instant::now(),
        });
    }

    fn transition_progress(&self) -> Option<f32> {
        self.transition.as_ref().map(|transition| {
            let elapsed = transition.started_at.elapsed();
            (elapsed.as_secs_f32() / TASK_MUX_TRANSITION_DURATION.as_secs_f32()).clamp(0.0, 1.0)
        })
    }

    fn finish_transition_if_ready(&mut self) {
        let Some(progress) = self.transition_progress() else {
            return;
        };
        if progress < 1.0 {
            return;
        }
        if let Some(transition) = self.transition.take() {
            self.mode = transition.to_mode;
            self.focused = transition.to_focus;
            self.selected = transition.to_focus;
        }
    }
}

/// The main chat widget displaying conversation history.
pub struct ChatWidget {
    /// History cells to display.
    pub cells: Vec<Box<dyn HistoryCell>>,
    /// Active DAG panel shown alongside history while a workflow is running.
    dag_panel: Option<DagPanelState>,
    /// Active task mux view for parallel task execution.
    task_mux: Option<TaskMuxState>,
    /// Number of lines scrolled up from the bottom. `0` means pinned to bottom.
    pub scroll_from_bottom_lines: usize,
    /// Bottom pane for input.
    pub bottom_pane: BottomPane,
    /// Last rendered input area rectangle (for mouse hit-testing).
    last_input_area: Option<Rect>,
    /// Last rendered chat area width used to estimate added line count.
    last_chat_area_width: u16,
}

impl ChatWidget {
    /// Create a new chat widget.
    pub fn new() -> Self {
        Self {
            cells: Vec::new(),
            dag_panel: None,
            task_mux: None,
            scroll_from_bottom_lines: 0,
            bottom_pane: BottomPane::new(),
            last_input_area: None,
            last_chat_area_width: 80,
        }
    }

    /// Add a cell to the history.
    pub fn add_cell(&mut self, cell: Box<dyn HistoryCell>) {
        if self.scroll_from_bottom_lines > 0 {
            self.scroll_from_bottom_lines = self
                .scroll_from_bottom_lines
                .saturating_add(cell.desired_height(self.last_chat_area_width) as usize);
        }
        self.cells.push(cell);
    }

    /// Insert a cell at a specific index.
    pub fn insert_cell(&mut self, index: usize, cell: Box<dyn HistoryCell>) {
        if self.scroll_from_bottom_lines > 0 {
            self.scroll_from_bottom_lines = self
                .scroll_from_bottom_lines
                .saturating_add(cell.desired_height(self.last_chat_area_width) as usize);
        }
        self.cells.insert(index, cell);
    }

    /// Scroll up by N lines.
    pub fn scroll_up_lines(&mut self, lines: usize) {
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.saturating_add(lines);
    }

    /// Scroll down by N lines.
    pub fn scroll_down_lines(&mut self, lines: usize) {
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.saturating_sub(lines);
    }

    /// Scroll to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom_lines = 0;
    }

    /// Scroll to the top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_from_bottom_lines = usize::MAX;
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.cells.clear();
        self.dag_panel = None;
        self.task_mux = None;
        self.scroll_from_bottom_lines = 0;
    }

    pub fn show_dag_panel(&mut self, plan: ExecutionPlanSpec) {
        self.task_mux = None;
        self.dag_panel = Some(DagPanelState::new(plan));
    }

    pub fn show_dag_preview(&mut self, plan: ExecutionPlanSpec) {
        self.task_mux = None;
        self.dag_panel = Some(DagPanelState::new(plan));
    }

    pub fn update_dag_task_status(&mut self, task_id: Uuid, status: TaskNodeStatus) {
        self.clear_stale_task_mux();
        if let Some(panel) = self.dag_panel.as_mut() {
            panel.update_task_status(task_id, status.clone());
        }
        if let Some(task_mux) = self.task_mux.as_mut() {
            task_mux.update_task_status(task_id, status);
        }
    }

    pub fn update_dag_progress(&mut self, completed: usize, total: usize) {
        if let Some(panel) = self.dag_panel.as_mut() {
            panel.update_progress(completed, total);
        }
    }

    pub fn update_dag_validation_status(&mut self, passed: bool) {
        if let Some(panel) = self.dag_panel.as_mut() {
            panel.update_validation_status(passed);
        }
    }

    pub fn update_dag_release_status(&mut self, passed: bool) {
        if let Some(panel) = self.dag_panel.as_mut() {
            panel.update_release_status(passed);
        }
    }

    pub fn clear_dag_panel(&mut self) {
        self.dag_panel = None;
        self.task_mux = None;
    }

    pub fn clear_task_mux(&mut self) {
        self.task_mux = None;
    }

    pub fn apply_task_runtime_event(&mut self, task_id: Uuid, event: TaskRuntimeEvent) {
        self.clear_stale_task_mux();
        if let Some(task_mux) = self.task_mux.as_mut() {
            task_mux.apply_runtime_event(task_id, event);
        }
    }

    pub fn interrupt_task_mux_tool_calls(&mut self) {
        if let Some(task_mux) = self.task_mux.as_mut() {
            task_mux.interrupt_running_tool_calls();
        }
    }

    pub fn has_task_mux(&self) -> bool {
        self.task_mux.is_some()
    }

    fn clear_stale_task_mux(&mut self) {
        let stale = match (&self.task_mux, &self.dag_panel) {
            (Some(task_mux), Some(dag_panel)) => task_mux.revision != dag_panel.revision,
            (Some(_), None) => true,
            _ => false,
        };
        if stale {
            self.task_mux = None;
        }
    }

    pub fn task_mux_next(&mut self) -> bool {
        let Some(task_mux) = self.task_mux.as_mut() else {
            return false;
        };
        task_mux.next();
        true
    }

    pub fn task_mux_prev(&mut self) -> bool {
        let Some(task_mux) = self.task_mux.as_mut() else {
            return false;
        };
        task_mux.prev();
        true
    }

    pub fn task_mux_toggle_mode(&mut self) -> bool {
        let Some(task_mux) = self.task_mux.as_mut() else {
            return false;
        };
        task_mux.toggle_mode();
        true
    }

    pub fn task_mux_show_overview(&mut self) -> bool {
        let Some(task_mux) = self.task_mux.as_mut() else {
            return false;
        };
        task_mux.show_overview();
        true
    }

    pub fn task_mux_focus_selected(&mut self) -> bool {
        let Some(task_mux) = self.task_mux.as_mut() else {
            return false;
        };
        task_mux.focus_selected();
        true
    }

    pub fn task_mux_focus_index(&mut self, index: usize) -> bool {
        let Some(task_mux) = self.task_mux.as_mut() else {
            return false;
        };
        task_mux.focus_index(index)
    }

    pub fn task_mux_context_label(&self) -> Option<String> {
        let task_mux = self.task_mux.as_ref()?;
        let task = task_mux.current_focus_task()?;
        let mode_label = match task_mux.mode {
            TaskMuxMode::Overview => "Overview",
            TaskMuxMode::Focused => "Focus",
        };
        let workspace_label = if task.isolated { "isolated" } else { "shared" };
        Some(format!(
            "Mux · {} · {:02} {} · {}",
            mode_label,
            task.ordinal,
            truncate_label(&task.title, 18),
            workspace_label
        ))
    }

    pub fn task_mux_list_lines(&self) -> Option<Vec<String>> {
        let task_mux = self.task_mux.as_ref()?;
        let focus_index = task_mux.current_focus_index();
        Some(
            task_mux
                .tasks
                .iter()
                .enumerate()
                .map(|(index, task)| {
                    let focus_marker = if index == focus_index { ">" } else { " " };
                    let workspace_label = if task.isolated { "isolated" } else { "shared" };
                    format!(
                        "{} {:02} {:<8} {:<10} {:<9} {}",
                        focus_marker,
                        task.ordinal,
                        task_kind_label(&task.kind),
                        task_status_label(&task.status),
                        workspace_label,
                        task.title
                    )
                })
                .collect(),
        )
    }

    pub fn task_mux_input_hint(&self) -> Option<String> {
        self.task_mux.as_ref().map(|task_mux| {
            let total = task_mux.tasks.len();
            format!(
                "Type /mux next, /mux prev, /mux focus <1-{}>, /mux overview",
                total
            )
        })
    }

    pub fn is_in_input_area(&self, x: u16, y: u16) -> bool {
        self.last_input_area.is_some_and(|rect| {
            x >= rect.x
                && x < rect.x.saturating_add(rect.width)
                && y >= rect.y
                && y < rect.y.saturating_add(rect.height)
        })
    }

    fn split_areas(&self, area: Rect) -> (Rect, Rect) {
        let bottom_height = self.bottom_pane.desired_height();
        let chunks = Layout::vertical([
            Constraint::Min(3),                // Chat area (min 3 lines)
            Constraint::Length(bottom_height), // Bottom pane (dynamic)
        ])
        .split(area);
        (chunks[0], chunks[1])
    }

    /// Render the chat widget.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let area = area.intersection(*buf.area());
        if area.width == 0 || area.height == 0 {
            self.last_input_area = None;
            return None;
        }

        let (chat_area, bottom_area) = self.split_areas(area);

        // Render chat area
        self.render_chat_area(chat_area, buf);

        // Track the active input area for mouse hit-testing.
        self.last_input_area = self.bottom_pane.input_hitbox(bottom_area);

        self.bottom_pane.render(bottom_area, buf)
    }

    pub fn chat_area_rect(&self, area: Rect) -> Rect {
        self.split_areas(area).0
    }

    pub fn render_bottom_pane_only(&mut self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let area = area.intersection(*buf.area());
        if area.width == 0 || area.height == 0 {
            self.last_input_area = None;
            return None;
        }

        let (_, bottom_area) = self.split_areas(area);
        self.last_input_area = self.bottom_pane.input_hitbox(bottom_area);
        self.bottom_pane.render(bottom_area, buf)
    }

    fn render_chat_area(&mut self, area: Rect, buf: &mut Buffer) {
        self.clear_stale_task_mux();
        if let Some(task_mux) = self.task_mux.as_mut() {
            task_mux.finish_transition_if_ready();
        }

        let (main_area, aux_area) = self.split_chat_layout(area);
        self.last_chat_area_width = main_area.width;

        if self.task_mux.is_some() {
            self.render_task_mux(main_area, buf);
            if let Some(aux_area) = aux_area {
                self.render_dag_panel(aux_area, buf);
            }
            return;
        }

        self.render_history_cells(main_area, buf);
        if let Some(aux_area) = aux_area {
            self.render_dag_panel(aux_area, buf);
        }
    }

    fn split_chat_layout(&self, area: Rect) -> (Rect, Option<Rect>) {
        let has_aux = self.task_mux.is_some() || self.dag_panel.is_some();
        if !has_aux {
            return (area, None);
        }

        if self.task_mux.is_some() {
            if area.width >= 108 {
                let chunks = Layout::horizontal([
                    Constraint::Percentage(100 - DAG_PANEL_WIDTH_PERCENT),
                    Constraint::Percentage(DAG_PANEL_WIDTH_PERCENT),
                ])
                .split(area);
                return (chunks[0], Some(chunks[1]));
            }
            return (area, None);
        }

        if area.width < 96 {
            return (area, None);
        }

        let chunks = Layout::horizontal([
            Constraint::Percentage(100 - DAG_PANEL_WIDTH_PERCENT),
            Constraint::Percentage(DAG_PANEL_WIDTH_PERCENT),
        ])
        .split(area);
        (chunks[0], Some(chunks[1]))
    }

    fn render_history_cells(&mut self, area: Rect, buf: &mut Buffer) {
        // Calculate visible lines.
        let mut lines: Vec<Line<'static>> = Vec::new();

        for cell in &self.cells {
            lines.extend(cell.display_lines(area.width));
        }

        let visible_lines = area.height as usize;
        let total_lines = lines.len();

        let max_scroll_from_bottom = total_lines.saturating_sub(visible_lines);
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.min(max_scroll_from_bottom);

        let start_line = total_lines
            .saturating_sub(visible_lines)
            .saturating_sub(self.scroll_from_bottom_lines);

        let text = Text::from(lines);
        ratatui::widgets::Paragraph::new(text)
            .scroll((start_line.min(u16::MAX as usize) as u16, 0))
            .render(area, buf);

        if total_lines > visible_lines {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let mut scrollbar_state = ScrollbarState::new(total_lines)
                .position(start_line)
                .viewport_content_length(visible_lines);

            ratatui::widgets::StatefulWidget::render(scrollbar, area, buf, &mut scrollbar_state);
        }
    }

    fn render_task_mux(&self, area: Rect, buf: &mut Buffer) {
        let Some(task_mux) = self.task_mux.as_ref() else {
            return;
        };
        if area.width < 18 || area.height < 8 {
            return;
        }

        let phase = animation_phase(95);
        let progress = task_mux.transition_progress();
        let transition_active = progress.is_some_and(|progress| progress < 1.0);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if transition_active {
                theme::interactive::in_progress()
            } else {
                theme::border::idle()
            })
            .title(Line::from(vec![
                Span::styled(
                    format!(" Mux r{} ", task_mux.revision),
                    theme::interactive::title(),
                ),
                Span::styled(
                    format!(" {} panes ", task_mux.tasks.len()),
                    theme::text::muted(),
                ),
                Span::styled(
                    match task_mux.mode {
                        TaskMuxMode::Overview => " overview ",
                        TaskMuxMode::Focused => " focus ",
                    },
                    theme::text::subtle().add_modifier(Modifier::DIM),
                ),
            ]))
            .title_alignment(Alignment::Center);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width < 12 || inner.height < 6 {
            return;
        }

        let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(inner);
        let header_rows =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(chunks[0]);
        self.render_task_mux_summary(header_rows[0], buf, task_mux, phase, transition_active);

        if let Some(focus_index) = task_mux.current_focus_task().map(|task| task.ordinal) {
            let summary = format!(
                "focus {:02}  tab next/prev  ctrl+o overview  ctrl+f focus  enter /mux",
                focus_index
            );
            Paragraph::new(Line::styled(
                truncate_label(&summary, header_rows[1].width as usize),
                theme::text::help(),
            ))
            .render(header_rows[1], buf);
        }

        let transition = task_mux.transition.clone();
        match (&task_mux.mode, transition.as_ref()) {
            (_, Some(transition)) if task_mux.tasks.len() > 1 => {
                self.render_task_mux_transition(chunks[1], buf, task_mux, transition);
            }
            (TaskMuxMode::Focused, _) => self.render_task_mux_focus(chunks[1], buf, task_mux),
            (TaskMuxMode::Overview, _) => self.render_task_mux_overview(chunks[1], buf, task_mux),
        }
    }

    fn render_task_mux_summary(
        &self,
        area: Rect,
        buf: &mut Buffer,
        task_mux: &TaskMuxState,
        phase: usize,
        transition_active: bool,
    ) {
        let running = task_mux
            .tasks
            .iter()
            .filter(|task| task.status == TaskNodeStatus::Running)
            .count();
        let completed = task_mux
            .tasks
            .iter()
            .filter(|task| task.status == TaskNodeStatus::Completed)
            .count();
        let failed = task_mux
            .tasks
            .iter()
            .filter(|task| task.status == TaskNodeStatus::Failed)
            .count();
        let label = format!(
            "● running {}  done {}  failed {}  layers {}",
            running,
            completed,
            failed,
            task_mux
                .tasks
                .iter()
                .map(|task| task.depth)
                .max()
                .map(|depth| depth + 1)
                .unwrap_or(0)
        );
        let line = if transition_active {
            gradient_line(&label, &theme::animation::executing_gradient(), phase, true)
        } else {
            Line::styled(label, theme::text::muted())
        };
        Paragraph::new(line).render(area, buf);
    }

    fn render_task_mux_transition(
        &self,
        area: Rect,
        buf: &mut Buffer,
        task_mux: &TaskMuxState,
        transition: &TaskMuxTransition,
    ) {
        let progress = task_mux.transition_progress().unwrap_or(1.0);
        match (transition.from_mode, transition.to_mode) {
            (TaskMuxMode::Overview, TaskMuxMode::Focused) => {
                self.render_task_mux_overview(area, buf, task_mux);
                if let Some(task) = task_mux.tasks.get(transition.to_focus) {
                    let start = overview_cell_rect(area, transition.to_focus, task_mux.tasks.len());
                    let overlay = lerp_rect(start, area, ease_out(progress));
                    self.render_task_window(
                        overlay,
                        buf,
                        task_mux,
                        task,
                        TaskWindowRenderState {
                            selected: true,
                            focused: true,
                            emphasis: progress,
                        },
                    );
                }
            }
            (TaskMuxMode::Focused, TaskMuxMode::Overview) => {
                self.render_task_mux_overview(area, buf, task_mux);
                if let Some(task) = task_mux.tasks.get(transition.from_focus) {
                    let end = overview_cell_rect(area, transition.from_focus, task_mux.tasks.len());
                    let overlay = lerp_rect(area, end, ease_out(progress));
                    self.render_task_window(
                        overlay,
                        buf,
                        task_mux,
                        task,
                        TaskWindowRenderState {
                            selected: true,
                            focused: false,
                            emphasis: 1.0 - progress,
                        },
                    );
                }
            }
            (_, _) => {
                self.render_task_mux_focus(area, buf, task_mux);
            }
        }
    }

    fn render_task_mux_overview(&self, area: Rect, buf: &mut Buffer, task_mux: &TaskMuxState) {
        let cells = split_overview_cells(area, task_mux.tasks.len());
        for (index, task) in task_mux.tasks.iter().enumerate() {
            let Some(cell) = cells.get(index).copied() else {
                break;
            };
            let is_selected = index == task_mux.selected;
            let is_focused = index == task_mux.focused && task_mux.mode == TaskMuxMode::Focused;
            self.render_task_window(
                cell,
                buf,
                task_mux,
                task,
                TaskWindowRenderState {
                    selected: is_selected,
                    focused: is_focused,
                    emphasis: 1.0,
                },
            );
        }
    }

    fn render_task_mux_focus(&self, area: Rect, buf: &mut Buffer, task_mux: &TaskMuxState) {
        let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(4)]).split(area);
        self.render_task_mux_tabs(chunks[0], buf, task_mux);
        if let Some(task) = task_mux.tasks.get(task_mux.current_focus_index()) {
            self.render_task_window(
                chunks[1],
                buf,
                task_mux,
                task,
                TaskWindowRenderState {
                    selected: true,
                    focused: true,
                    emphasis: 1.0,
                },
            );
        }
    }

    fn render_task_mux_tabs(&self, area: Rect, buf: &mut Buffer, task_mux: &TaskMuxState) {
        let phase = animation_phase(100);
        let spans = task_mux
            .tasks
            .iter()
            .enumerate()
            .flat_map(|(index, task)| {
                let mut style = if index == task_mux.current_focus_index() {
                    theme::interactive::selected_option()
                } else {
                    theme::text::muted()
                };
                if task.status == TaskNodeStatus::Running {
                    style = Style::default()
                        .fg(theme::animation::active_gradient()
                            [(index + phase) % theme::animation::active_gradient().len()])
                        .add_modifier(Modifier::BOLD);
                }
                [
                    Span::styled(
                        format!(" {:02} {} ", task.ordinal, truncate_label(&task.title, 12)),
                        style,
                    ),
                    Span::raw(" "),
                ]
            })
            .collect::<Vec<_>>();
        Paragraph::new(Line::from(spans)).render(area, buf);
    }

    fn render_task_window(
        &self,
        area: Rect,
        buf: &mut Buffer,
        task_mux: &TaskMuxState,
        task: &TaskMuxTaskState,
        render_state: TaskWindowRenderState,
    ) {
        if area.width < 12 || area.height < 4 {
            return;
        }

        let phase = animation_phase(90);
        let running = task.status == TaskNodeStatus::Running;
        let mut border_style = if render_state.focused || render_state.selected {
            theme::border::focused()
        } else {
            theme::border::idle()
        };
        if running {
            border_style = Style::default().fg(theme::animation::executing_gradient()
                [(task.ordinal + phase) % theme::animation::executing_gradient().len()]);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Line::from(vec![
                Span::styled(
                    format!(" {:02} ", task.ordinal),
                    panel_node_style(&task.status),
                ),
                Span::styled(
                    truncate_label(&task.title, area.width.saturating_sub(12) as usize),
                    if render_state.focused {
                        theme::interactive::selected_option()
                    } else {
                        theme::text::primary()
                    },
                ),
            ]));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width < 4 || inner.height < 2 {
            return;
        }

        let mut lines = Vec::new();
        let mode_label = task
            .workspace_backend
            .map(TaskWorkspaceBackend::label)
            .unwrap_or(if task.isolated { "isolated" } else { "shared" });
        lines.push(Line::from(vec![
            Span::styled(task_kind_label(&task.kind), task_kind_style(&task.kind)),
            Span::raw(" "),
            Span::styled(
                task_status_label(&task.status),
                panel_node_style(&task.status),
            ),
            Span::styled(format!(" · {}", mode_label), theme::text::muted()),
            Span::styled(" · ", theme::text::muted()),
            task_phase_span(&task.phase, task.ordinal, area.width),
        ]));
        if let Some(working_dir) = task.working_dir.as_ref() {
            let display_dir = if task.isolated {
                task.main_working_dir.as_ref().unwrap_or(working_dir)
            } else {
                working_dir
            };
            let dir_label = display_dir
                .file_name()
                .and_then(|part| part.to_str())
                .unwrap_or_else(|| display_dir.as_os_str().to_str().unwrap_or("."));
            let prefix = if task.isolated { "source" } else { "cwd" };
            lines.push(Line::styled(
                format!(
                    "{} {}",
                    prefix,
                    truncate_label(
                        dir_label,
                        inner.width.saturating_sub(prefix.len() as u16 + 1) as usize
                    )
                ),
                theme::text::subtle(),
            ));
        }

        let available_log_lines = inner.height.saturating_sub(lines.len() as u16) as usize;
        if available_log_lines > 0 {
            let rendered_logs =
                render_task_transcript_lines(&task.transcript, inner.width, available_log_lines);
            lines.extend(rendered_logs);
        }

        if lines.is_empty() {
            lines.push(Line::styled("idle", theme::text::subtle()));
        }

        let style = if render_state.emphasis < 1.0 {
            theme::text::muted().add_modifier(Modifier::DIM)
        } else {
            Style::default()
        };
        Paragraph::new(Text::from(lines))
            .style(style)
            .render(inner, buf);

        if render_state.selected
            && task_mux.mode == TaskMuxMode::Overview
            && inner.width > 2
            && let Some(cell) = buf.cell_mut((inner.x, inner.y))
        {
            cell.set_symbol("›")
                .set_style(theme::interactive::selected_option());
        }
    }

    fn render_dag_panel(&self, area: Rect, buf: &mut Buffer) {
        let Some(panel) = self.dag_panel.as_ref() else {
            return;
        };
        if area.width < 18 || area.height < 6 {
            return;
        }

        let title_style = if panel.has_running() {
            theme::interactive::in_progress()
        } else {
            theme::interactive::title()
        };
        let title = Line::from(vec![
            Span::styled("⌁ ", theme::interactive::accent()),
            Span::styled("Workflow · ", title_style.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("Plan r{}", panel.revision),
                title_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {} / {}", panel.completed, panel.total),
                theme::text::muted(),
            ),
        ]);
        Paragraph::new(title).render(Rect::new(area.x, area.y, area.width, 1), buf);

        // Context block: intent / plan lineage / replan reason. Sits between
        // the title and the "Thread graph" heading.
        let context_area_top = area.y.saturating_add(1);
        let context_lines = panel.context_lines(area.width);
        let context_height = (context_lines.len() as u16).min(area.height.saturating_sub(1));
        if context_height > 0 {
            let context_rect = Rect::new(area.x, context_area_top, area.width, context_height);
            Paragraph::new(Text::from(context_lines)).render(context_rect, buf);
        }

        // Layout: title (1 row) + context (N rows) + blank gap + heading + blank + graph rows.
        let heading_y = context_area_top
            .saturating_add(context_height)
            .saturating_add(1);
        if heading_y >= area.bottom() {
            return;
        }
        Paragraph::new(Line::styled(
            "Thread graph",
            theme::text::primary().add_modifier(Modifier::BOLD),
        ))
        .render(Rect::new(area.x, heading_y, area.width, 1), buf);

        let rows_y = heading_y.saturating_add(2);
        if rows_y >= area.bottom() {
            return;
        }
        let rows_area = Rect::new(area.x, rows_y, area.width, area.bottom() - rows_y);
        if rows_area.height == 0 {
            return;
        }
        let rows = workflow_branch_rows(panel, rows_area.height as usize);
        render_workflow_branch_rows(rows_area, buf, &rows);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowBranchLane {
    /// Trunk row (intent/plan/phase/validation/release). Renders on lane 0
    /// with continuation through the multi-lane region below.
    Main,
    /// Task row, using `WorkflowBranchRow::lane` as a column index alongside
    /// the trunk. `lanes_in_layer` reports the fan width for the depth layer
    /// so the renderer can draw connecting edges.
    Task,
    /// Synthetic row that draws the fan-in connector merging an N-lane layer
    /// back to the trunk before the next stage. Reserves its own grid row so
    /// it never overlaps with a labelled node.
    LayerMerge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowBranchStatus {
    Complete,
    Active,
    Pending,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
struct WorkflowBranchRow {
    lane: WorkflowBranchLane,
    status: WorkflowBranchStatus,
    label: String,
    /// Column within the depth layer (0-based). Only meaningful for `Task`.
    lane_index: usize,
    /// Fan width of the depth layer this row belongs to. Only meaningful for
    /// `Task`. Used by the renderer to draw fan-out / fan-in edges.
    lanes_in_layer: usize,
    /// First/last task row marker for the layer. Used by the renderer to
    /// position fan-out / fan-in glyphs.
    layer_position: LayerPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum LayerPosition {
    #[default]
    None,
    First,
    Middle,
    Last,
    Solo,
}

fn workflow_branch_rows(panel: &DagPanelState, max_rows: usize) -> Vec<WorkflowBranchRow> {
    if max_rows == 0 {
        return Vec::new();
    }

    let lane_count = panel.lane_count();
    let layer_count = panel.depth_count();
    let mut rows = Vec::new();
    rows.push(WorkflowBranchRow {
        lane: WorkflowBranchLane::Main,
        status: WorkflowBranchStatus::Complete,
        label: "intent: confirm".to_string(),
        lane_index: 0,
        lanes_in_layer: 0,
        layer_position: LayerPosition::None,
    });
    let plan_label = if panel.nodes.is_empty() {
        "plan: exec + test".to_string()
    } else {
        format!(
            "plan: exec + test · {} lanes · {} layers",
            lane_count.max(1),
            layer_count.max(1)
        )
    };
    rows.push(WorkflowBranchRow {
        lane: WorkflowBranchLane::Main,
        status: WorkflowBranchStatus::Complete,
        label: plan_label,
        lane_index: 0,
        lanes_in_layer: 0,
        layer_position: LayerPosition::None,
    });
    rows.push(WorkflowBranchRow {
        lane: WorkflowBranchLane::Main,
        status: workflow_phase2_status(panel),
        label: "phase 2: start".to_string(),
        lane_index: 0,
        lanes_in_layer: 0,
        layer_position: LayerPosition::None,
    });

    let reserve_for_terminal_rows = 2usize;
    let base_task_capacity = max_rows.saturating_sub(rows.len() + reserve_for_terminal_rows);
    // If we will truncate, drop one slot so the `"... N more tasks"` overflow
    // marker can actually be emitted; the previous capacity exactly equalled
    // the budget and the overflow guard below was therefore unreachable.
    let will_omit = panel.nodes.len() > base_task_capacity;
    let task_capacity = if will_omit && base_task_capacity > 1 {
        base_task_capacity - 1
    } else {
        base_task_capacity
    };
    // Reserve the saved slot for the overflow marker, so the synthetic
    // `LayerMerge` row below cannot eat it before the marker is pushed.
    let reserve_for_overflow_marker = usize::from(will_omit && base_task_capacity > 1);
    let visible_task_count = panel.nodes.len().min(task_capacity);
    // Layer-aware ordering: render tasks grouped by depth (top-to-bottom),
    // then by lane (left-to-right) so multi-lane layers stay contiguous even
    // when the underlying plan declared tasks in interleaved order.
    let mut depth_sorted: Vec<&DagPanelNode> = panel.nodes.iter().collect();
    depth_sorted.sort_by_key(|node| (node.depth, node.lane, node.ordinal));
    let visible_nodes: Vec<&DagPanelNode> =
        depth_sorted.into_iter().take(visible_task_count).collect();
    // Recompute lane widths from the visible subset, not the full panel
    // state. Otherwise a row budget that truncates a parallel layer mid-fan
    // would still draw connectors and ghost-lane glyphs for tasks that are
    // not actually being rendered.
    let visible_lanes_at_depth: std::collections::HashMap<usize, usize> = {
        let mut per_depth: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for node in &visible_nodes {
            *per_depth.entry(node.depth).or_default() += 1;
        }
        per_depth
    };
    let id_to_ordinal: std::collections::HashMap<Uuid, usize> = panel
        .nodes
        .iter()
        .map(|node| (node.task_id, node.ordinal))
        .collect();
    for (idx, node) in visible_nodes.iter().enumerate() {
        let dep_suffix = if node.dependency_count == 0 {
            String::new()
        } else {
            // Show explicit predecessor ordinals (`dep 01,02`) instead of just a
            // count so reviewers can trace the DAG without expanding the row.
            let mut deps: Vec<usize> = node
                .dependencies
                .iter()
                .filter_map(|dep| id_to_ordinal.get(dep).copied())
                .collect();
            deps.sort_unstable();
            if deps.is_empty() {
                format!(" · dep {}", node.dependency_count)
            } else {
                let joined = deps
                    .iter()
                    .map(|ord| format!("{:02}", ord))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(" · dep {joined}")
            }
        };
        let lanes = visible_lanes_at_depth
            .get(&node.depth)
            .copied()
            .unwrap_or(1)
            .max(1);
        let prev_depth = if idx == 0 {
            None
        } else {
            Some(visible_nodes[idx - 1].depth)
        };
        let next_depth = visible_nodes.get(idx + 1).map(|next| next.depth);
        let layer_position = match (prev_depth, next_depth) {
            (prev, next) if prev != Some(node.depth) && next != Some(node.depth) => {
                if lanes == 1 {
                    LayerPosition::Solo
                } else {
                    LayerPosition::First
                }
            }
            (prev, _) if prev != Some(node.depth) => LayerPosition::First,
            (_, next) if next != Some(node.depth) => LayerPosition::Last,
            _ => LayerPosition::Middle,
        };
        rows.push(WorkflowBranchRow {
            lane: WorkflowBranchLane::Task,
            status: workflow_status_from_task(&node.status),
            label: format!(
                "{}{:02} {}{}",
                task_kind_prefix(&node.kind),
                node.ordinal,
                node.title,
                dep_suffix
            ),
            lane_index: node.lane,
            lanes_in_layer: lanes,
            layer_position,
        });

        // After the last task in a multi-lane layer, reserve a synthetic
        // merge row so the fan-in glyphs don't overwrite the next stage. We
        // only push the merge if the running budget still has room for: the
        // merge row itself, all REMAINING visible task rows below this layer,
        // the validation/release stages, and the overflow marker (when
        // truncation is pending). Otherwise `truncate(max_rows)` below would
        // silently drop downstream task rows, terminal stages, or the
        // overflow marker.
        let remaining_visible_task_rows = visible_nodes.len().saturating_sub(idx + 1);
        if matches!(layer_position, LayerPosition::Last)
            && lanes > 1
            && rows.len()
                + 1
                + remaining_visible_task_rows
                + reserve_for_terminal_rows
                + reserve_for_overflow_marker
                <= max_rows
        {
            rows.push(WorkflowBranchRow {
                lane: WorkflowBranchLane::LayerMerge,
                status: WorkflowBranchStatus::Pending,
                label: String::new(),
                lane_index: 0,
                lanes_in_layer: lanes,
                layer_position: LayerPosition::None,
            });
        }
    }

    let omitted = panel.nodes.len().saturating_sub(visible_task_count);
    if omitted > 0 && rows.len() + reserve_for_terminal_rows < max_rows {
        rows.push(WorkflowBranchRow {
            lane: WorkflowBranchLane::Task,
            status: WorkflowBranchStatus::Pending,
            label: format!("... {omitted} more tasks"),
            lane_index: 0,
            lanes_in_layer: 1,
            layer_position: LayerPosition::Solo,
        });
    }

    rows.push(WorkflowBranchRow {
        lane: WorkflowBranchLane::Main,
        status: workflow_validation_status(panel),
        label: "validation".to_string(),
        lane_index: 0,
        lanes_in_layer: 0,
        layer_position: LayerPosition::None,
    });
    rows.push(WorkflowBranchRow {
        lane: WorkflowBranchLane::Main,
        status: workflow_stage_status(panel.release_status),
        label: "release".to_string(),
        lane_index: 0,
        lanes_in_layer: 0,
        layer_position: LayerPosition::None,
    });
    rows.truncate(max_rows);
    rows
}

fn workflow_phase2_status(panel: &DagPanelState) -> WorkflowBranchStatus {
    if panel.failed_count() > 0 {
        WorkflowBranchStatus::Failed
    } else if panel.has_running() {
        WorkflowBranchStatus::Active
    } else if panel.total > 0 && panel.completed >= panel.total {
        WorkflowBranchStatus::Complete
    } else {
        WorkflowBranchStatus::Pending
    }
}

fn workflow_validation_status(panel: &DagPanelState) -> WorkflowBranchStatus {
    match panel.validation_status {
        DagStageStatus::Complete | DagStageStatus::Failed => {
            return workflow_stage_status(panel.validation_status);
        }
        DagStageStatus::Pending => {}
    }

    if panel.failed_count() > 0 {
        WorkflowBranchStatus::Failed
    } else if panel.total > 0 && panel.completed >= panel.total {
        WorkflowBranchStatus::Active
    } else {
        WorkflowBranchStatus::Pending
    }
}

fn workflow_stage_status(status: DagStageStatus) -> WorkflowBranchStatus {
    match status {
        DagStageStatus::Pending => WorkflowBranchStatus::Pending,
        DagStageStatus::Complete => WorkflowBranchStatus::Complete,
        DagStageStatus::Failed => WorkflowBranchStatus::Failed,
    }
}

fn workflow_status_from_task(status: &TaskNodeStatus) -> WorkflowBranchStatus {
    match status {
        TaskNodeStatus::Pending => WorkflowBranchStatus::Pending,
        TaskNodeStatus::Running => WorkflowBranchStatus::Active,
        TaskNodeStatus::Completed => WorkflowBranchStatus::Complete,
        TaskNodeStatus::Failed => WorkflowBranchStatus::Failed,
        TaskNodeStatus::Skipped => WorkflowBranchStatus::Skipped,
    }
}

/// Spacing between adjacent task lanes (number of columns each lane occupies).
const LANE_STRIDE: u16 = 3;

fn render_workflow_branch_rows(area: Rect, buf: &mut Buffer, rows: &[WorkflowBranchRow]) {
    if rows.is_empty() || area.width < 8 {
        return;
    }

    let main_x = area.x.saturating_add(2);
    let max_lanes = rows
        .iter()
        .filter(|row| row.lane == WorkflowBranchLane::Task)
        .map(|row| row.lanes_in_layer)
        .max()
        .unwrap_or(0)
        .max(1);
    // First task lane sits 3 columns to the right of the trunk.
    let branch_origin = main_x.saturating_add(3);
    // Width consumed by lane glyphs alone: max_lanes lanes × stride, minus
    // trailing slack so the label still fits.
    let lane_columns = (max_lanes as u16).saturating_mul(LANE_STRIDE);
    let label_origin = branch_origin
        .saturating_add(lane_columns.saturating_sub(LANE_STRIDE.saturating_sub(1)))
        .min(area.right().saturating_sub(1));

    // Indices of the first and last task-or-merge rows so we can keep the
    // in-layer lane lines visible across all rows that span the task block
    // (including synthetic merge markers).
    let first_branch = rows.iter().position(|row| {
        matches!(
            row.lane,
            WorkflowBranchLane::Task | WorkflowBranchLane::LayerMerge
        )
    });
    let last_branch = rows.iter().rposition(|row| {
        matches!(
            row.lane,
            WorkflowBranchLane::Task | WorkflowBranchLane::LayerMerge
        )
    });
    let trunk_style = theme::text::subtle();
    let branch_style = theme::interactive::accent();

    for (index, row) in rows.iter().enumerate() {
        let y = area.y.saturating_add(index as u16);
        if y >= area.bottom() {
            break;
        }

        // Draw the trunk line on every interior row so rows above and below
        // visually connect. We draw `│` first and let the node glyph below
        // overwrite it where appropriate. Clip to the panel area so we never
        // touch a column owned by an adjacent panel.
        if rows.len() > 1 && main_x < area.right() {
            set_branch_cell(buf, main_x, y, "│", trunk_style);
        }

        // While we are inside the task block, keep all in-layer lanes drawn
        // through this row so multi-row layers stay visually connected. Skip
        // for the LayerMerge row since it draws its own connectors.
        if let (Some(first), Some(last)) = (first_branch, last_branch)
            && row.lane == WorkflowBranchLane::Task
            && index >= first
            && index <= last
        {
            for lane_idx in 0..row.lanes_in_layer {
                let lane_x = lane_x_for(branch_origin, lane_idx);
                if lane_x < area.right() {
                    set_branch_cell(buf, lane_x, y, "│", branch_style);
                }
            }
        }

        match row.lane {
            WorkflowBranchLane::Main => {
                set_branch_cell(
                    buf,
                    main_x,
                    y,
                    workflow_branch_glyph(row.status),
                    workflow_branch_style(row.status).add_modifier(Modifier::BOLD),
                );
                write_branch_label(
                    buf,
                    main_x.saturating_add(4),
                    y,
                    area.right(),
                    &row.label,
                    row.status,
                );
            }
            WorkflowBranchLane::Task => {
                let lane_x = lane_x_for(branch_origin, row.lane_index);
                if lane_x < area.right() {
                    // Draw a fan-out connector on the row that opens the
                    // layer, joining the trunk to all lanes for this layer.
                    if matches!(
                        row.layer_position,
                        LayerPosition::First | LayerPosition::Solo
                    ) {
                        set_branch_cell(buf, main_x, y, "├", branch_style);
                        let last_lane_x =
                            lane_x_for(branch_origin, row.lanes_in_layer.saturating_sub(1));
                        // Continuous horizontal bridge from the trunk to the
                        // far-right lane, clipped to the panel area so we
                        // never bleed into adjacent panes. Lane glyphs are
                        // overlaid afterwards.
                        let bridge_end = last_lane_x.min(area.right().saturating_sub(1));
                        for x in main_x.saturating_add(1)..=bridge_end {
                            set_branch_cell(buf, x, y, "─", branch_style);
                        }
                        if row.lanes_in_layer > 1 {
                            for lane_idx in 0..row.lanes_in_layer {
                                let mid_x = lane_x_for(branch_origin, lane_idx);
                                if mid_x >= area.right() {
                                    break;
                                }
                                let glyph = if lane_idx == 0 && row.lane_index == 0 {
                                    workflow_branch_glyph(row.status)
                                } else if lane_idx == row.lanes_in_layer - 1 {
                                    "╮"
                                } else {
                                    "┬"
                                };
                                let style = if lane_idx == 0 && row.lane_index == 0 {
                                    workflow_branch_style(row.status).add_modifier(Modifier::BOLD)
                                } else {
                                    branch_style
                                };
                                set_branch_cell(buf, mid_x, y, glyph, style);
                            }
                        }
                    }

                    // Always render the node glyph for this row's lane.
                    set_branch_cell(
                        buf,
                        lane_x,
                        y,
                        workflow_branch_glyph(row.status),
                        workflow_branch_style(row.status).add_modifier(Modifier::BOLD),
                    );

                    let label_x = lane_x.saturating_add(2).max(label_origin);
                    write_branch_label(buf, label_x, y, area.right(), &row.label, row.status);
                }
            }
            WorkflowBranchLane::LayerMerge => {
                if row.lanes_in_layer > 1 {
                    set_branch_cell(buf, main_x, y, "├", branch_style);
                    let last_lane_x = lane_x_for(branch_origin, row.lanes_in_layer - 1);
                    // Bridge clipped to the panel area so the merge connector
                    // never overwrites the column to the right of the panel.
                    let bridge_end = last_lane_x.min(area.right().saturating_sub(1));
                    for x in main_x.saturating_add(1)..=bridge_end {
                        set_branch_cell(buf, x, y, "─", branch_style);
                    }
                    for lane_idx in 0..row.lanes_in_layer {
                        let mid_x = lane_x_for(branch_origin, lane_idx);
                        if mid_x >= area.right() {
                            break;
                        }
                        let glyph = if lane_idx == row.lanes_in_layer - 1 {
                            "╯"
                        } else {
                            "┴"
                        };
                        set_branch_cell(buf, mid_x, y, glyph, branch_style);
                    }
                }
            }
        }
    }
}

fn lane_x_for(origin: u16, lane_index: usize) -> u16 {
    origin.saturating_add((lane_index as u16).saturating_mul(LANE_STRIDE))
}

fn set_branch_cell(buf: &mut Buffer, x: u16, y: u16, symbol: &str, style: Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_symbol(symbol).set_style(style);
    }
}

fn write_branch_label(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    right: u16,
    label: &str,
    status: WorkflowBranchStatus,
) {
    if x >= right {
        return;
    }
    let width = right.saturating_sub(x) as usize;
    let style = match status {
        WorkflowBranchStatus::Complete => theme::text::primary(),
        WorkflowBranchStatus::Active => theme::interactive::selected_option(),
        WorkflowBranchStatus::Pending => theme::text::muted(),
        WorkflowBranchStatus::Failed => theme::status::danger(),
        WorkflowBranchStatus::Skipped => theme::text::subtle(),
    };
    for (offset, ch) in truncate_label(label, width).chars().enumerate() {
        let cell_x = x.saturating_add(offset as u16);
        if cell_x >= right {
            break;
        }
        if let Some(cell) = buf.cell_mut((cell_x, y)) {
            cell.set_symbol(ch.encode_utf8(&mut [0; 4]))
                .set_style(style);
        }
    }
}

fn workflow_branch_glyph(status: WorkflowBranchStatus) -> &'static str {
    match status {
        WorkflowBranchStatus::Complete => "●",
        WorkflowBranchStatus::Active => "●",
        WorkflowBranchStatus::Pending => "○",
        WorkflowBranchStatus::Failed => "×",
        WorkflowBranchStatus::Skipped => "○",
    }
}

fn workflow_branch_style(status: WorkflowBranchStatus) -> Style {
    match status {
        WorkflowBranchStatus::Complete => theme::text::primary(),
        WorkflowBranchStatus::Active => theme::interactive::accent(),
        WorkflowBranchStatus::Pending => theme::text::subtle(),
        WorkflowBranchStatus::Failed => theme::status::danger(),
        WorkflowBranchStatus::Skipped => theme::text::muted(),
    }
}

fn split_overview_cells(area: Rect, count: usize) -> Vec<Rect> {
    if count == 0 || area.width < 12 || area.height < 4 {
        return Vec::new();
    }
    let columns = if count == 1 {
        1
    } else if area.width >= 56 {
        2
    } else {
        1
    };
    let rows = count.div_ceil(columns);
    if rows == 0 {
        return Vec::new();
    }

    let row_constraints = vec![Constraint::Ratio(1, rows as u32); rows];
    let row_chunks = Layout::vertical(row_constraints).split(area);
    let mut rects = Vec::new();
    for row in row_chunks.iter().copied() {
        let col_constraints = vec![Constraint::Ratio(1, columns as u32); columns];
        let cols = Layout::horizontal(col_constraints).split(row);
        rects.extend(cols.iter().copied());
    }
    rects.truncate(count);
    rects
}

fn overview_cell_rect(area: Rect, index: usize, count: usize) -> Rect {
    split_overview_cells(area, count)
        .get(index)
        .copied()
        .unwrap_or(area)
}

fn lerp_rect(from: Rect, to: Rect, progress: f32) -> Rect {
    let lerp = |start: u16, end: u16| -> u16 {
        let start = start as f32;
        let end = end as f32;
        (start + (end - start) * progress)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16
    };
    Rect {
        x: lerp(from.x, to.x),
        y: lerp(from.y, to.y),
        width: lerp(from.width.max(1), to.width.max(1)).max(1),
        height: lerp(from.height.max(1), to.height.max(1)).max(1),
    }
}

fn ease_out(progress: f32) -> f32 {
    let inv = 1.0 - progress.clamp(0.0, 1.0);
    1.0 - inv * inv
}

fn gradient_line(text: &str, colors: &[Color], phase: usize, bold: bool) -> Line<'static> {
    let spans = text
        .chars()
        .enumerate()
        .map(|(idx, ch)| {
            let color = colors[(idx + phase) % colors.len()];
            let mut style = Style::default().fg(color);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            Span::styled(ch.to_string(), style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn gradient_span(text: &str, colors: &[Color], phase: usize, bold: bool) -> Span<'static> {
    Span::styled(
        text.to_string(),
        Style::default()
            .fg(colors[phase % colors.len()])
            .add_modifier(if bold {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    )
}

fn push_task_transcript_entry(
    transcript: &mut Vec<TaskMuxTranscriptEntry>,
    entry: TaskMuxTranscriptEntry,
) {
    if transcript.len() >= TASK_MUX_MAX_ENTRIES {
        let remove = transcript.len() + 1 - TASK_MUX_MAX_ENTRIES;
        transcript.drain(0..remove);
    }
    transcript.push(entry);
}

fn complete_streaming_task_thinking(transcript: &mut [TaskMuxTranscriptEntry]) {
    for entry in transcript.iter_mut() {
        if let TaskMuxTranscriptEntry::Thinking(cell) = entry {
            cell.complete();
        }
    }
}

fn render_task_transcript_lines(
    transcript: &[TaskMuxTranscriptEntry],
    width: u16,
    max_lines: usize,
) -> Vec<Line<'static>> {
    if transcript.is_empty() || width == 0 || max_lines == 0 {
        return vec![Line::styled("waiting for activity", theme::text::subtle())];
    }

    let mut all_lines = Vec::new();
    for entry in transcript {
        match entry {
            TaskMuxTranscriptEntry::Note(entry) => {
                let (prefix, style) = match entry.level {
                    TaskRuntimeNoteLevel::Info => ("· ", theme::text::muted()),
                    TaskRuntimeNoteLevel::Error => ("! ", theme::status::danger()),
                };
                all_lines.extend(wrap_mux_text(&entry.text, prefix, width, style));
            }
            TaskMuxTranscriptEntry::Thinking(cell) => {
                all_lines.extend(cell.display_lines(width));
            }
            TaskMuxTranscriptEntry::Assistant(cell) => {
                all_lines.extend(cell.display_lines(width));
            }
            TaskMuxTranscriptEntry::Tool(cell) => {
                all_lines.extend(cell.display_lines(width));
            }
        }
    }

    if all_lines.len() <= max_lines {
        return all_lines;
    }

    all_lines.split_off(all_lines.len() - max_lines)
}

fn task_phase_span(phase: &TaskRuntimePhase, ordinal: usize, width: u16) -> Span<'static> {
    match phase {
        TaskRuntimePhase::Pending => Span::styled("pending", theme::text::subtle()),
        TaskRuntimePhase::Starting => Span::styled("starting", theme::text::muted()),
        TaskRuntimePhase::AwaitingModel { turn } => {
            let phase_ix = animation_phase(110);
            let label = format!("thinking t{turn}");
            gradient_span(
                &truncate_label(&label, width.saturating_sub(24) as usize),
                &theme::animation::active_gradient(),
                ordinal + phase_ix,
                true,
            )
        }
        TaskRuntimePhase::ExecutingTool { tool_name } => Span::styled(
            truncate_label(
                &format!("tool {}", tool_name),
                width.saturating_sub(24) as usize,
            ),
            theme::interactive::accent(),
        ),
        TaskRuntimePhase::Reviewing => Span::styled("reviewing", theme::interactive::accent()),
        TaskRuntimePhase::Completed => Span::styled("complete", theme::status::success()),
        TaskRuntimePhase::Failed => Span::styled("failed", theme::status::danger()),
    }
}

fn wrap_mux_text(text: &str, prefix: &str, width: u16, style: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let total_cols = (width as usize).max(8);
    let prefix_cols = prefix.chars().count();
    let cont_prefix = " ".repeat(prefix_cols);

    for logical_line in text.lines() {
        let mut remaining = logical_line;
        let mut first = true;
        loop {
            let pfx: &str = if first { prefix } else { &cont_prefix };
            let available = total_cols.saturating_sub(prefix_cols).max(1);
            let char_count = remaining.chars().count();
            if char_count <= available {
                out.push(Line::styled(format!("{pfx}{remaining}"), style));
                break;
            }
            let split_byte = remaining
                .char_indices()
                .nth(available)
                .map(|(i, _)| i)
                .unwrap_or(remaining.len());
            out.push(Line::styled(
                format!("{pfx}{}", &remaining[..split_byte]),
                style,
            ));
            remaining = &remaining[split_byte..];
            first = false;
        }
    }

    if out.is_empty() {
        out.push(Line::styled(prefix.to_string(), style));
    }

    out
}

fn task_kind_label(kind: &TaskKind) -> &'static str {
    match kind {
        TaskKind::Implementation => "impl",
        TaskKind::Analysis => "analysis",
        TaskKind::Gate => "gate",
    }
}

fn task_kind_prefix(kind: &TaskKind) -> &'static str {
    match kind {
        TaskKind::Implementation => "I",
        TaskKind::Analysis => "A",
        TaskKind::Gate => "G",
    }
}

fn task_kind_style(kind: &TaskKind) -> Style {
    match kind {
        TaskKind::Implementation => theme::interactive::accent(),
        TaskKind::Analysis => theme::text::muted(),
        TaskKind::Gate => theme::status::warning(),
    }
}

fn task_status_label(status: &TaskNodeStatus) -> &'static str {
    match status {
        TaskNodeStatus::Pending => "pending",
        TaskNodeStatus::Running => "running",
        TaskNodeStatus::Completed => "done",
        TaskNodeStatus::Failed => "failed",
        TaskNodeStatus::Skipped => "skipped",
    }
}

fn panel_node_style(status: &TaskNodeStatus) -> Style {
    match status {
        TaskNodeStatus::Pending => theme::text::subtle(),
        TaskNodeStatus::Running => theme::interactive::in_progress(),
        TaskNodeStatus::Completed => theme::status::success(),
        TaskNodeStatus::Failed => theme::status::danger(),
        TaskNodeStatus::Skipped => theme::text::muted(),
    }
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }

    let mut truncated = String::new();
    for ch in text.chars().take(max_chars.saturating_sub(1)) {
        truncated.push(ch);
    }
    truncated.push('…');
    truncated
}

impl Default for ChatWidget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};
    use ratatui::{buffer::Buffer, layout::Rect};
    use serde_json::json;

    use super::{
        ChatWidget, DagPanelState, LayerPosition, TaskMuxTranscriptEntry, WorkflowBranchLane,
        WorkflowBranchRow, WorkflowBranchStatus, render_task_transcript_lines,
        workflow_branch_rows,
    };
    use crate::internal::{
        ai::orchestrator::types::{
            ExecutionPlanSpec, TaskContract, TaskKind, TaskNodeStatus, TaskRuntimeEvent, TaskSpec,
        },
        tui::{
            history_cell::{AssistantHistoryCell, ThinkingHistoryCell},
            welcome_shader,
        },
    };

    fn row_text(buf: &Buffer, y: u16, width: u16) -> String {
        let mut out = String::new();
        for x in 0..width {
            out.push_str(buf[(x, y)].symbol());
        }
        out
    }

    #[test]
    fn initial_welcome_render_clamps_to_reported_release_buffer_size() {
        let buffer_area = Rect::new(0, 0, 122, 35);
        let oversized_frame_area = Rect::new(0, 0, 122, 37);
        let mut buf = Buffer::empty(buffer_area);
        let mut widget = ChatWidget::new();
        widget
            .bottom_pane
            .set_cwd(std::path::PathBuf::from("/Volumes/Data/linked"));
        widget.bottom_pane.set_git_branch(Some("main".to_string()));

        let chat_area = widget.chat_area_rect(oversized_frame_area);
        let welcome = welcome_shader::WelcomeView {
            welcome_message: "Welcome to Libra Code!\nWeb: http://127.0.0.1:3000\nMCP: http://127.0.0.1:6789",
            model_name: "glm-5.1:cloud",
            provider_name: "ollama",
            cwd: std::path::Path::new("/Volumes/Data/linked"),
        };

        welcome_shader::render(chat_area, &mut buf, welcome);
        let _ = widget.render_bottom_pane_only(oversized_frame_area, &mut buf);
    }

    fn make_task(title: &str, kind: TaskKind, dependencies: Vec<uuid::Uuid>) -> TaskSpec {
        let actor = ActorRef::agent("dag-panel-test").unwrap();
        let mut task = GitTask::new(actor, title, None).unwrap();
        for dependency in dependencies {
            task.add_dependency(dependency);
        }
        TaskSpec {
            step: git_internal::internal::object::plan::PlanStep::new(title),
            task,
            objective: title.to_string(),
            kind,
            gate_stage: None,
            owner_role: None,
            scope_in: vec![],
            scope_out: vec![],
            checks: vec![],
            contract: TaskContract::default(),
        }
    }

    fn sample_plan() -> ExecutionPlanSpec {
        let first = make_task(
            "Analyze repository structure",
            TaskKind::Implementation,
            vec![],
        );
        let second = make_task("Fast gate", TaskKind::Gate, vec![first.id()]);
        ExecutionPlanSpec {
            intent_spec_id: "intent-1".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![first, second],
            max_parallel: 1,
            checkpoints: vec![],
        }
    }

    fn sample_parallel_plan() -> ExecutionPlanSpec {
        let first = make_task("Implement A", TaskKind::Implementation, vec![]);
        let second = make_task("Implement B", TaskKind::Implementation, vec![]);
        let gate = make_task("Fast gate", TaskKind::Gate, vec![first.id(), second.id()]);
        ExecutionPlanSpec {
            intent_spec_id: "intent-par".into(),
            revision: 2,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![first, second, gate],
            max_parallel: 2,
            checkpoints: vec![],
        }
    }

    fn sample_serial_replan(revision: u32) -> ExecutionPlanSpec {
        let first = make_task("Inspect replan", TaskKind::Implementation, vec![]);
        let second = make_task("Verify replan", TaskKind::Gate, vec![first.id()]);
        ExecutionPlanSpec {
            intent_spec_id: "intent-replan".into(),
            revision,
            parent_revision: Some(revision.saturating_sub(1)),
            replan_reason: Some("parallel task failed".into()),
            tasks: vec![first, second],
            max_parallel: 1,
            checkpoints: vec![],
        }
    }

    #[test]
    fn dag_panel_uses_side_column_when_wide() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_plan());

        let (history, dag) = widget.split_chat_layout(Rect::new(0, 0, 120, 30));
        let dag = dag.unwrap();

        assert_eq!(history.width + dag.width, 120);
        assert_eq!(dag.width, 42);
        assert!(history.width < 120);
    }

    #[test]
    fn dag_panel_uses_thirty_five_percent_side_column() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_parallel_plan());

        let (history, dag) = widget.split_chat_layout(Rect::new(0, 0, 140, 32));
        let dag = dag.unwrap();

        assert_eq!(history.width, 91);
        assert_eq!(dag.width, 49);
    }

    #[test]
    fn dag_panel_hides_when_narrow() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_plan());

        let (history, dag) = widget.split_chat_layout(Rect::new(0, 0, 80, 24));

        assert_eq!(history.width, 80);
        assert!(dag.is_none());
    }

    #[test]
    fn dag_preview_does_not_activate_task_mux() {
        let mut widget = ChatWidget::new();
        widget.show_dag_preview(sample_parallel_plan());

        let (main, sidebar) = widget.split_chat_layout(Rect::new(0, 0, 140, 32));

        assert!(!widget.has_task_mux());
        assert!(sidebar.is_some());
        assert!(main.width < 140);
    }

    #[test]
    fn execution_workflow_keeps_chat_main_area_and_branch_sidebar() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_parallel_plan());

        let (main, sidebar) = widget.split_chat_layout(Rect::new(0, 0, 140, 32));

        assert!(!widget.has_task_mux());
        assert!(sidebar.is_some());
        assert!(main.width < 140);
        assert!(main.width > sidebar.unwrap().width);
    }

    #[test]
    fn task_mux_focus_command_is_inactive_in_chat_mode() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_parallel_plan());

        assert!(!widget.task_mux_focus_index(1));
        assert!(widget.task_mux_context_label().is_none());
    }

    #[test]
    fn task_mux_list_is_absent_in_chat_mode() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_parallel_plan());

        assert!(widget.task_mux_list_lines().is_none());
    }

    #[test]
    fn parallel_workflow_renders_chat_main_area_and_branch_sidebar() {
        let plan = sample_parallel_plan();
        let first_task_id = plan.tasks[0].id();

        let mut widget = ChatWidget::new();
        widget.add_cell(Box::new(AssistantHistoryCell::new(
            "transcript sentinel".to_string(),
        )));
        widget.show_dag_panel(plan);
        widget.update_dag_task_status(first_task_id, TaskNodeStatus::Running);
        widget.apply_task_runtime_event(
            first_task_id,
            TaskRuntimeEvent::ToolCallBegin {
                call_id: "call-1".to_string(),
                tool_name: "shell".to_string(),
                arguments: json!({
                    "command": "cargo test",
                    "workdir": "/tmp/task-1"
                }),
            },
        );

        let area = Rect::new(0, 0, 140, 32);
        let mut buf = Buffer::empty(area);
        widget.render_chat_area(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Workflow"));
        assert!(rendered.contains("Plan r2"));
        assert!(rendered.contains("Thread graph"));
        assert!(rendered.contains("phase 2: start"));
        assert!(rendered.contains("I01 Implement A"));
        assert!(rendered.contains("transcript sentinel"));
        assert!(!rendered.contains("Mux r2"));
    }

    #[test]
    fn task_transcript_renders_thinking_deltas() {
        let mut cell = ThinkingHistoryCell::streaming();
        cell.append("checking the failed plan step");
        let transcript = vec![TaskMuxTranscriptEntry::Thinking(cell)];

        let rendered = render_task_transcript_lines(&transcript, 80, 10)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Think"));
        assert!(rendered.contains("checking the failed plan step"));
    }

    #[test]
    fn serial_replan_keeps_chat_mode_and_updates_workflow_sidebar() {
        let mut widget = ChatWidget::new();
        widget.add_cell(Box::new(AssistantHistoryCell::new(
            "new revision transcript".to_string(),
        )));
        widget.show_dag_panel(sample_parallel_plan());
        assert!(!widget.has_task_mux());

        widget.show_dag_panel(sample_serial_replan(3));

        let area = Rect::new(0, 0, 140, 32);
        let mut buf = Buffer::empty(area);
        widget.render_chat_area(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!widget.has_task_mux());
        assert!(rendered.contains("Plan r3"));
        assert!(rendered.contains("new revision transcript"));
        assert!(!rendered.contains("Mux r2"));
        assert!(!rendered.contains("Implement A"));
    }

    #[test]
    fn clearing_task_mux_keeps_workflow_dag_visible() {
        let mut widget = ChatWidget::new();
        widget.add_cell(Box::new(AssistantHistoryCell::new(
            "verification stage".to_string(),
        )));
        widget.show_dag_panel(sample_parallel_plan());

        widget.clear_task_mux();

        let area = Rect::new(0, 0, 140, 32);
        let mut buf = Buffer::empty(area);
        widget.render_chat_area(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!widget.has_task_mux());
        assert!(rendered.contains("Plan r2"));
        assert!(rendered.contains("verification stage"));
        assert!(!rendered.contains("Mux r2"));
    }

    #[test]
    fn dag_panel_renders_graph_with_task_details() {
        let plan = sample_plan();
        let first_task_id = plan.tasks[0].id();

        let mut widget = ChatWidget::new();
        widget.show_dag_panel(plan);
        widget.update_dag_task_status(first_task_id, TaskNodeStatus::Completed);
        widget.update_dag_progress(1, 2);

        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        widget.render_chat_area(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Plan r1"));
        assert!(rendered.contains("1 / 2"));
        assert!(rendered.contains("Thread graph"));
        assert!(rendered.contains("plan: exec + test"));
        assert!(rendered.contains('●'));
        assert!(rendered.contains('○'));
        assert!(rendered.contains('│') || rendered.contains('─'));
        assert!(rendered.contains("I01 Analyze repository"));
        assert!(rendered.contains("G02 Fast gate"));
    }

    #[test]
    fn dag_panel_title_progress_tracks_terminal_task_statuses() {
        let plan = sample_plan();
        let first_task_id = plan.tasks[0].id();

        let mut widget = ChatWidget::new();
        widget.show_dag_panel(plan);
        widget.update_dag_task_status(first_task_id, TaskNodeStatus::Completed);

        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        widget.render_chat_area(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Plan r1"));
        assert!(rendered.contains("1 / 2"));
    }

    #[test]
    fn dag_panel_title_progress_counts_running_task_as_active() {
        let plan = sample_plan();
        let first_task_id = plan.tasks[0].id();

        let mut widget = ChatWidget::new();
        widget.show_dag_panel(plan);
        widget.update_dag_task_status(first_task_id, TaskNodeStatus::Running);
        widget.update_dag_progress(0, 2);

        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        widget.render_chat_area(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Plan r1"));
        assert!(rendered.contains("1 / 2"));
    }

    #[test]
    fn dag_panel_terminal_rows_track_validation_and_release_status() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_plan());

        let panel = widget.dag_panel.as_ref().unwrap();
        let rows = workflow_branch_rows(panel, 10);
        let validation = rows.iter().find(|row| row.label == "validation").unwrap();
        let release = rows.iter().find(|row| row.label == "release").unwrap();
        assert_eq!(validation.status, WorkflowBranchStatus::Pending);
        assert_eq!(release.status, WorkflowBranchStatus::Pending);

        widget.update_dag_validation_status(true);
        widget.update_dag_release_status(true);

        let panel = widget.dag_panel.as_ref().unwrap();
        let rows = workflow_branch_rows(panel, 10);
        let validation = rows.iter().find(|row| row.label == "validation").unwrap();
        let release = rows.iter().find(|row| row.label == "release").unwrap();
        assert_eq!(validation.status, WorkflowBranchStatus::Complete);
        assert_eq!(release.status, WorkflowBranchStatus::Complete);
    }

    #[test]
    fn single_node_layer_renders_as_branch_row() {
        let first = make_task("A1", TaskKind::Analysis, vec![]);
        let second = make_task("A2", TaskKind::Analysis, vec![]);
        let third = make_task("Gate", TaskKind::Gate, vec![first.id(), second.id()]);
        let plan = ExecutionPlanSpec {
            intent_spec_id: "intent-1".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![first, second, third],
            max_parallel: 2,
            checkpoints: vec![],
        };

        let mut widget = ChatWidget::new();
        widget.show_dag_panel(plan);

        let area = Rect::new(0, 0, 42, 24);
        let mut buf = Buffer::empty(area);
        widget.render_dag_panel(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Thread graph"));
        assert!(rendered.contains("Plan r1"));
        assert!(rendered.contains("G03 Gate"));
    }

    #[test]
    fn dag_panel_renders_intent_and_plan_lineage_in_context_block() {
        let plan = sample_serial_replan(3);
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(plan);

        let area = Rect::new(0, 0, 60, 24);
        let mut buf = Buffer::empty(area);
        widget.render_dag_panel(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            rendered.contains("intent intent-replan"),
            "rendered=\n{rendered}"
        );
        assert!(rendered.contains("plan r3 ← r2"), "rendered=\n{rendered}");
        assert!(
            rendered.contains("replan: parallel task failed"),
            "rendered=\n{rendered}"
        );
        assert!(rendered.contains("parallel 1"), "rendered=\n{rendered}");
    }

    #[test]
    fn overflow_marker_appears_when_tasks_exceed_budget() {
        // 5 tasks but only enough room to render 2 — the renderer must emit
        // a visible "... 3 more tasks" marker rather than silently dropping
        // the rest.
        let plan = ExecutionPlanSpec {
            intent_spec_id: "intent-overflow".into(),
            revision: 7,
            parent_revision: None,
            replan_reason: None,
            tasks: (0..5)
                .map(|idx| make_task(&format!("Step {}", idx + 1), TaskKind::Analysis, vec![]))
                .collect(),
            max_parallel: 1,
            checkpoints: vec![],
        };
        let panel = DagPanelState::new(plan);
        // 8 rows = 3 main + 2 task + 1 overflow + 2 terminal.
        let rows = workflow_branch_rows(&panel, 8);
        assert!(
            rows.iter().any(|row| row.label == "... 3 more tasks"),
            "expected overflow marker, got {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.label == "validation"),
            "validation row dropped, got {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.label == "release"),
            "release row dropped, got {rows:?}"
        );
    }

    #[test]
    fn overflow_marker_survives_competing_layer_merge_row() {
        // Two parallel implementation tasks at depth 0 plus three sequential
        // analyses at depths 1..=3. The merge guard must yield to the
        // overflow marker AND leave room for any remaining downstream task
        // rows so neither validation/release nor the marker is dropped.
        let parallel_a = make_task("Parallel A", TaskKind::Implementation, vec![]);
        let parallel_b = make_task("Parallel B", TaskKind::Implementation, vec![]);
        let mid = make_task(
            "Mid",
            TaskKind::Analysis,
            vec![parallel_a.id(), parallel_b.id()],
        );
        let late = make_task("Late", TaskKind::Analysis, vec![mid.id()]);
        let last = make_task("Last", TaskKind::Analysis, vec![late.id()]);
        let plan = ExecutionPlanSpec {
            intent_spec_id: "intent-merge-vs-overflow".into(),
            revision: 9,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![parallel_a, parallel_b, mid, late, last],
            max_parallel: 2,
            checkpoints: vec![],
        };
        let panel = DagPanelState::new(plan);

        // Two budgets that previously failed in different ways: an 8-row
        // budget exposes the original LayerMerge-vs-overflow conflict, and a
        // 9-row budget exposes the remaining-task-rows variant where the
        // merge eats the overflow slot via downstream task pressure.
        for budget in [8usize, 9] {
            let rows = workflow_branch_rows(&panel, budget);
            assert!(
                rows.iter().any(|row| row.label == "validation"),
                "validation dropped at budget {budget}, got {rows:?}"
            );
            assert!(
                rows.iter().any(|row| row.label == "release"),
                "release dropped at budget {budget}, got {rows:?}"
            );
            assert!(
                rows.iter()
                    .any(|row| row.label.starts_with("...") && row.label.contains("more tasks")),
                "overflow marker dropped at budget {budget}, got {rows:?}"
            );
        }
    }

    #[test]
    fn truncated_parallel_layer_only_draws_visible_lanes() {
        // Build a 3-lane parallel layer where the row budget only admits the
        // first task. The renderer must not draw connector glyphs for the
        // two ghost lanes.
        let first = make_task("Implement A", TaskKind::Implementation, vec![]);
        let second = make_task("Implement B", TaskKind::Implementation, vec![]);
        let third = make_task("Implement C", TaskKind::Implementation, vec![]);
        let plan = ExecutionPlanSpec {
            intent_spec_id: "intent-trim".into(),
            revision: 4,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![first, second, third],
            max_parallel: 3,
            checkpoints: vec![],
        };
        let panel = DagPanelState::new(plan);

        // 6 row budget = intent + plan + phase + 1 task + validation + release.
        let rows = workflow_branch_rows(&panel, 6);
        let task_rows: Vec<&WorkflowBranchRow> = rows
            .iter()
            .filter(|row| row.lane == WorkflowBranchLane::Task)
            .collect();
        assert_eq!(task_rows.len(), 1, "rows = {rows:?}");
        // The lone visible task must be marked as a Solo layer (no fan-out
        // glyphs) because no other lanes are being rendered alongside it.
        let only_task = task_rows[0];
        assert_eq!(only_task.lanes_in_layer, 1);
        assert_eq!(only_task.layer_position, LayerPosition::Solo);
        // No synthetic LayerMerge row should slip in either.
        assert!(
            !rows
                .iter()
                .any(|row| row.lane == WorkflowBranchLane::LayerMerge)
        );
    }

    #[test]
    fn parallel_layer_fans_out_into_multiple_lanes() {
        let plan = sample_parallel_plan();
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(plan);

        let panel = widget.dag_panel.as_ref().unwrap();
        let rows = workflow_branch_rows(panel, 32);
        let task_layer: Vec<&WorkflowBranchRow> = rows
            .iter()
            .filter(|row| row.lane == WorkflowBranchLane::Task)
            .collect();
        // Two parallel implementation tasks at depth 0, one gate at depth 1.
        assert!(
            task_layer
                .iter()
                .any(|row| row.lane_index == 1 && row.lanes_in_layer == 2)
        );
        assert_eq!(
            task_layer
                .iter()
                .filter(|row| row.lanes_in_layer == 2)
                .count(),
            2
        );

        let area = Rect::new(0, 0, 60, 32);
        let mut buf = Buffer::empty(area);
        widget.render_dag_panel(area, &mut buf);
        let row_lines: Vec<String> = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect();
        let rendered = row_lines.join("\n");

        // Pin the exact column layout of the fan-out row so a regression that
        // dropped the lane-0 → lane-1 bridge segment (cols 6,7) cannot pass.
        // Layout: main_x=2, branch_origin=5, LANE_STRIDE=3, lanes 0/1 at 5/8.
        let fanout = row_lines
            .iter()
            .find(|line| line.contains('╮'))
            .expect("fan-out row should contain ╮");
        let fanout_chars: Vec<char> = fanout.chars().collect();
        assert_eq!(
            &fanout_chars[2..=8],
            &['├', '─', '─', '○', '─', '─', '╮'][..],
            "fan-out columns mismatch, got:\n{rendered}"
        );

        let fanin = row_lines
            .iter()
            .find(|line| line.contains('╯'))
            .expect("fan-in row should contain ╯");
        let fanin_chars: Vec<char> = fanin.chars().collect();
        assert_eq!(
            &fanin_chars[2..=8],
            &['├', '─', '─', '┴', '─', '─', '╯'][..],
            "fan-in columns mismatch, got:\n{rendered}"
        );

        // Dependency suffix should reference the upstream task ordinals.
        assert!(rendered.contains("dep 01,02") || rendered.contains("dep 02,01"));
    }
}
