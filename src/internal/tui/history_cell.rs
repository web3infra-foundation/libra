//! History cells for the chat transcript.
//!
//! A `HistoryCell` is the unit of display in the conversation UI, representing
//! user messages, assistant responses, and tool calls.

use std::{
    any::Any,
    collections::{BTreeMap, HashMap},
    fmt::Debug,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use ratatui::prelude::*;
use serde_json::Value;
use uuid::Uuid;

use super::{
    diff::{DiffSummary, FileChange, create_diff_summary},
    markdown_render::render_markdown_lines,
    theme,
};
use crate::internal::ai::{
    orchestrator::types::{
        DecisionOutcome, ExecutionPlanSpec, GateReport, OrchestratorResult, PersistedExecution,
        TaskKind, TaskNodeStatus,
    },
    tools::{
        ToolOutput,
        context::{PlanStep, StepStatus},
    },
};

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    if max_bytes == 0 {
        return String::new();
    }

    let mut end = 0usize;
    for (idx, ch) in text.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }

    text[..end].to_string()
}

fn animation_phase(step_ms: u128) -> usize {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    (millis / step_ms.max(1)) as usize
}

fn gradient_line(text: &str, colors: &[Color], phase: usize, bold: bool) -> Line<'static> {
    let spans = text
        .chars()
        .enumerate()
        .map(|(idx, ch)| {
            let color = colors[(idx + phase) % colors.len()];
            let mut style = Style::default().fg(color);
            if bold {
                style = style.bold();
            }
            Span::styled(ch.to_string(), style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// Wrap `text` into styled ratatui `Line`s, splitting at `width` columns.
///
/// `prefix` is prepended to the first segment of every logical line.
/// Continuation segments (when wrapping occurs) receive a blank indent of
/// the same display width so the text stays aligned.
fn wrap_text(text: &str, prefix: &str, width: u16, style: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    // Guard against unreasonably small widths.
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
            // Split at the character boundary that fits within `available` columns.
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

    // Produce at least one line so the caller always gets something.
    if out.is_empty() {
        out.push(Line::styled(prefix.to_string(), style));
    }

    out
}

/// Trait for cells displayed in the chat history.
pub trait HistoryCell: Debug + Send + Sync {
    /// Render the cell as lines for display.
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

    /// Calculate the desired height for the cell.
    fn desired_height(&self, width: u16) -> u16 {
        let lines = self.display_lines(width);
        lines.len() as u16
    }

    /// Downcast to concrete type for mutation.
    fn as_any(&self) -> &dyn Any;

    /// Downcast to concrete type for mutation.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// A user message in the chat history.
#[derive(Debug, Clone)]
pub struct UserHistoryCell {
    /// The user's message text.
    pub message: String,
}

impl UserHistoryCell {
    /// Create a new user history cell.
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl HistoryCell for UserHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        for line in self.message.lines() {
            lines.extend(wrap_text(line, "│ ", width, theme::interactive::accent()));
        }

        lines.push(Line::raw("")); // Empty line for spacing
        lines
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// An assistant message in the chat history.
#[derive(Debug, Clone)]
pub struct AssistantHistoryCell {
    /// The assistant's response content.
    pub content: String,
    /// Whether the response is still streaming.
    pub is_streaming: bool,
}

impl AssistantHistoryCell {
    /// Create a new assistant history cell.
    pub fn new(content: String) -> Self {
        Self {
            content,
            is_streaming: false,
        }
    }

    /// Create a streaming assistant history cell.
    pub fn streaming() -> Self {
        Self {
            content: String::new(),
            is_streaming: true,
        }
    }

    /// Mark the response as complete.
    pub fn complete(&mut self) {
        self.is_streaming = false;
    }
}

impl HistoryCell for AssistantHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        let content = self.content.trim();
        if !content.is_empty() {
            let rendered = render_markdown_lines(content, width.saturating_sub(2));
            for (idx, line) in rendered.into_iter().enumerate() {
                let prefix = if idx == 0 { "● " } else { "  " };
                let mut spans = vec![Span::raw(prefix.to_string())];
                spans.extend(line.spans);
                lines.push(Line::from(spans).style(line.style));
            }
        }

        if self.is_streaming && !content.is_empty() {
            lines.push(Line::styled("  ▌", theme::status::ready()));
        } else if !self.is_streaming {
            lines.push(Line::raw("")); // Empty line for spacing
        }

        lines
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A tool call in the chat history.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolCallGroup {
    Explore,
    Edit,
    Shell,
    Input,
    Draft,
    Other(String),
}

impl ToolCallGroup {
    fn for_tool(tool_name: &str) -> Self {
        match tool_name {
            "read_file" | "list_dir" | "grep_files" => Self::Explore,
            "apply_patch" => Self::Edit,
            "shell" => Self::Shell,
            "request_user_input" => Self::Input,
            "submit_intent_draft" => Self::Draft,
            _ => Self::Other(tool_name.to_string()),
        }
    }

    fn labels(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Explore => ("Exploring", "Explored", "Explore failed"),
            Self::Edit => ("Editing", "Edited", "Edit failed"),
            Self::Shell => ("Running command", "Ran command", "Command failed"),
            Self::Input => ("Waiting for input", "Input received", "Input cancelled"),
            Self::Draft => ("Drafting", "Drafted", "Draft failed"),
            Self::Other(_) => ("Working", "Completed", "Failed"),
        }
    }

    fn action_style(&self) -> Style {
        match self {
            Self::Explore => theme::tool::explore(),
            Self::Edit => theme::tool::edit(),
            Self::Shell => theme::tool::shell(),
            Self::Input => theme::tool::input(),
            Self::Draft => theme::tool::draft(),
            Self::Other(_) => theme::text::subtle(),
        }
    }
}

#[derive(Debug, Clone)]
enum ToolCallEntryStatus {
    Running,
    Success,
    Failed(String),
}

#[derive(Debug, Clone)]
struct ToolCallEntry {
    call_id: String,
    summary: String,
    status: ToolCallEntryStatus,
}

#[derive(Debug, Clone)]
pub struct ToolCallHistoryCell {
    group: ToolCallGroup,
    entries: Vec<ToolCallEntry>,
}

impl ToolCallHistoryCell {
    /// Create a new tool call cell.
    pub fn new(call_id: String, tool_name: String, arguments: Value) -> Self {
        Self {
            group: ToolCallGroup::for_tool(&tool_name),
            entries: vec![ToolCallEntry {
                call_id,
                summary: summarize_tool_call(&tool_name, &arguments),
                status: ToolCallEntryStatus::Running,
            }],
        }
    }

    pub fn can_merge(&self, tool_name: &str) -> bool {
        self.group == ToolCallGroup::for_tool(tool_name)
    }

    pub fn append_call(&mut self, call_id: String, tool_name: String, arguments: Value) {
        self.entries.push(ToolCallEntry {
            call_id,
            summary: summarize_tool_call(&tool_name, &arguments),
            status: ToolCallEntryStatus::Running,
        });
    }

    pub fn contains_call_id(&self, call_id: &str) -> bool {
        self.entries.iter().any(|entry| entry.call_id == call_id)
    }

    /// Complete a single tool call inside the group.
    pub fn complete_call(&mut self, call_id: &str, result: Result<ToolOutput, String>) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.call_id == call_id)
        {
            entry.status = match result {
                Ok(output) if output.is_success() => ToolCallEntryStatus::Success,
                Ok(output) => ToolCallEntryStatus::Failed(summarize_tool_output_failure(&output)),
                Err(err) => ToolCallEntryStatus::Failed(err),
            };
        }
    }

    pub fn interrupt_running(&mut self) {
        for entry in &mut self.entries {
            if matches!(entry.status, ToolCallEntryStatus::Running) {
                entry.status = ToolCallEntryStatus::Failed("Interrupted".to_string());
            }
        }
    }

    pub fn has_running(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry.status, ToolCallEntryStatus::Running))
    }

    pub fn is_success(&self) -> bool {
        !self.has_running()
            && self
                .entries
                .iter()
                .all(|entry| matches!(entry.status, ToolCallEntryStatus::Success))
    }

    fn has_failure(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry.status, ToolCallEntryStatus::Failed(_)))
    }
}

impl HistoryCell for ToolCallHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let phase = animation_phase(120);
        let (running_label, done_label, failed_label) = self.group.labels();

        // Tool state summary line
        let summary = if self.has_running() {
            running_label
        } else {
            if self.has_failure() {
                failed_label
            } else {
                done_label
            }
        };
        if self.has_running() {
            lines.push(gradient_line(
                &format!("● {summary}"),
                &theme::animation::active_gradient(),
                phase,
                true,
            ));
        } else {
            let status_color = if self.is_success() {
                theme::status::success_color()
            } else {
                theme::status::danger_color()
            };
            lines.push(Line::styled(
                format!("● {summary}"),
                Style::default().fg(status_color).bold(),
            ));
        }

        for (idx, entry) in self.entries.iter().enumerate() {
            let prefix = if idx + 1 == self.entries.len() {
                "  └ "
            } else {
                "  ├ "
            };
            lines.extend(wrap_tool_entry(
                &entry.summary,
                prefix,
                width,
                self.group.action_style(),
            ));

            if let ToolCallEntryStatus::Failed(error) = &entry.status {
                lines.extend(wrap_text(
                    &truncate_utf8(error.trim(), 180),
                    "    ",
                    width,
                    theme::status::danger().add_modifier(Modifier::DIM),
                ));
            }
        }

        lines.push(Line::raw("")); // Empty line for spacing
        lines
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn summarize_tool_call(tool_name: &str, arguments: &Value) -> String {
    match tool_name {
        "read_file" => format!(
            "Read {}",
            argument_string(arguments, "file_path").unwrap_or("?")
        ),
        "list_dir" => format!(
            "List {}",
            argument_string(arguments, "dir_path").unwrap_or(".")
        ),
        "grep_files" => {
            let pattern = argument_string(arguments, "pattern").unwrap_or("(pattern)");
            let path = argument_string(arguments, "path").unwrap_or(".");
            format!(
                "Search {} in {}",
                truncate_utf8(pattern, 80),
                truncate_utf8(path, 80)
            )
        }
        "shell" => format!(
            "Run {}",
            truncate_utf8(
                argument_string(arguments, "command").unwrap_or("(command)"),
                120
            )
        ),
        "apply_patch" => summarize_apply_patch(arguments),
        "request_user_input" => "Ask for input".to_string(),
        "submit_intent_draft" => "Submit intent draft".to_string(),
        _ => format!("Run {}", tool_name.replace('_', " ")),
    }
}

fn summarize_apply_patch(arguments: &Value) -> String {
    let patch_text = arguments
        .as_str()
        .or_else(|| argument_string(arguments, "input"))
        .or_else(|| argument_string(arguments, "patch"));

    let Some(patch_text) = patch_text else {
        return "Apply patch".to_string();
    };

    let mut files = Vec::new();
    for line in patch_text.lines() {
        let file = line
            .strip_prefix("*** Update File: ")
            .or_else(|| line.strip_prefix("*** Add File: "))
            .or_else(|| line.strip_prefix("*** Delete File: "))
            .or_else(|| line.strip_prefix("*** Move to: "));
        if let Some(file) = file {
            files.push(file.trim().to_string());
        }
    }

    match files.as_slice() {
        [] => "Apply patch".to_string(),
        [file] => format!("Edit {file}"),
        [first, second] => format!("Edit {first} and {second}"),
        [first, rest @ ..] => format!("Edit {first} (+{} more)", rest.len()),
    }
}

fn summarize_tool_output_failure(output: &ToolOutput) -> String {
    match output {
        ToolOutput::Function { content, .. } => first_non_empty_line(content)
            .map(|line| truncate_utf8(line, 180))
            .unwrap_or_else(|| "Tool failed".to_string()),
        ToolOutput::Mcp { .. } => "MCP tool failed".to_string(),
    }
}

fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn argument_string<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(Value::as_str)
}

fn wrap_tool_entry(
    summary: &str,
    prefix: &str,
    width: u16,
    action_style: Style,
) -> Vec<Line<'static>> {
    let (action, detail) = summary
        .split_once(' ')
        .map_or((summary, ""), |(action, detail)| (action, detail));

    if detail.is_empty() {
        return vec![Line::from(vec![
            Span::styled(prefix.to_string(), theme::text::primary()),
            Span::styled(action.to_string(), action_style),
        ])];
    }

    let total_cols = (width as usize).max(8);
    let prefix_cols = prefix.chars().count();
    let action_cols = action.chars().count();
    let first_available = total_cols
        .saturating_sub(prefix_cols + action_cols + 1)
        .max(1);
    let continuation_prefix = " ".repeat(prefix_cols + action_cols + 1);
    let continuation_available = total_cols
        .saturating_sub(continuation_prefix.chars().count())
        .max(1);
    let detail_chunks = wrap_plain_chunks(detail, first_available, continuation_available);

    let mut lines = Vec::with_capacity(detail_chunks.len());
    if let Some((first, rest)) = detail_chunks.split_first() {
        lines.push(Line::from(vec![
            Span::styled(prefix.to_string(), theme::text::primary()),
            Span::styled(action.to_string(), action_style),
            Span::styled(format!(" {first}"), theme::text::primary()),
        ]));

        for chunk in rest {
            lines.push(Line::from(vec![Span::styled(
                format!("{continuation_prefix}{chunk}"),
                theme::text::primary(),
            )]));
        }
    }

    lines
}

fn wrap_plain_chunks(text: &str, first_width: usize, continuation_width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = text;
    let mut available = first_width.max(1);

    loop {
        let char_count = remaining.chars().count();
        if char_count <= available {
            chunks.push(remaining.to_string());
            break;
        }

        let split_byte = remaining
            .char_indices()
            .nth(available)
            .map(|(idx, _)| idx)
            .unwrap_or(remaining.len());
        chunks.push(remaining[..split_byte].to_string());
        remaining = &remaining[split_byte..];
        available = continuation_width.max(1);
    }

    if chunks.is_empty() {
        chunks.push(String::new());
    }

    chunks
}

/// A diff/patch display cell in the chat history.
#[derive(Debug, Clone)]
pub struct DiffHistoryCell {
    /// The diff summary to display.
    pub summary: DiffSummary,
}

impl DiffHistoryCell {
    /// Create a new diff history cell.
    pub fn new(changes: HashMap<PathBuf, FileChange>, cwd: PathBuf) -> Self {
        Self {
            summary: DiffSummary::new(changes, cwd),
        }
    }
}

impl HistoryCell for DiffHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::styled("● Diff", theme::text::primary().bold())];
        lines.extend(create_diff_summary(
            &self.summary.changes,
            &self.summary.cwd,
            width as usize,
        ));
        lines
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A plan update displayed as a checkbox list.
#[derive(Debug, Clone)]
pub struct PlanUpdateHistoryCell {
    /// Optional explanation from the model.
    pub explanation: Option<String>,
    /// The plan steps with their statuses.
    pub steps: Vec<PlanStep>,
    /// Whether the tool call is still running.
    pub is_running: bool,
    /// Unique id for this tool call.
    pub call_id: String,
}

impl PlanUpdateHistoryCell {
    /// Create a new plan update cell.
    pub fn new(call_id: String, explanation: Option<String>, steps: Vec<PlanStep>) -> Self {
        Self {
            explanation,
            steps,
            is_running: true,
            call_id,
        }
    }

    /// Mark the tool call as complete.
    pub fn complete(&mut self) {
        self.is_running = false;
    }
}

impl HistoryCell for PlanUpdateHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Header
        let status_icon = if self.is_running { "⏳" } else { "✓" };
        let status_color = if self.is_running {
            theme::status::warning_color()
        } else {
            theme::status::success_color()
        };
        lines.push(Line::from(vec![
            Span::styled("● ", theme::text::primary().bold()),
            Span::styled(
                format!("Plan {}:", status_icon),
                Style::default().fg(status_color).bold(),
            ),
        ]));

        // Optional explanation
        if let Some(ref explanation) = self.explanation {
            lines.extend(wrap_text(
                explanation,
                "  ",
                width,
                Style::default().add_modifier(Modifier::DIM).italic(),
            ));
        }

        // Steps with checkboxes
        for step in &self.steps {
            let (icon, style) = match step.status {
                StepStatus::Completed => (
                    "✔",
                    Style::default()
                        .add_modifier(Modifier::DIM)
                        .add_modifier(Modifier::CROSSED_OUT),
                ),
                StepStatus::InProgress => ("◐", theme::interactive::in_progress()),
                StepStatus::Pending => ("□", Style::default().add_modifier(Modifier::DIM)),
            };

            lines.extend(wrap_text(
                &format!("{} {}", icon, step.step),
                "  ",
                width,
                style,
            ));
        }

        lines.push(Line::raw("")); // Spacing
        lines
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
struct DagNodeEntry {
    task_id: Uuid,
    title: String,
    kind: TaskKind,
    dependencies: Vec<Uuid>,
    lane: usize,
    depth: usize,
    ordinal: usize,
    status: TaskNodeStatus,
}

#[derive(Debug, Clone, Default)]
struct DagGridCell {
    mask: u8,
    animated: bool,
}

#[derive(Debug, Clone)]
pub struct DagHistoryCell {
    revision: u32,
    summary: String,
    nodes: Vec<DagNodeEntry>,
    completed: usize,
    total: usize,
}

impl DagHistoryCell {
    pub fn new(plan: ExecutionPlanSpec) -> Self {
        let groups = plan.parallel_groups();
        let mut id_to_depth = HashMap::new();
        let mut id_to_lane = HashMap::new();
        for (depth, group) in groups.iter().enumerate() {
            for (lane, id) in group.iter().enumerate() {
                id_to_depth.insert(*id, depth);
                id_to_lane.insert(*id, lane);
            }
        }

        let nodes = plan
            .tasks
            .iter()
            .enumerate()
            .map(|(idx, task)| DagNodeEntry {
                task_id: task.id(),
                title: task.title().to_string(),
                kind: task.kind.clone(),
                dependencies: task.dependencies().to_vec(),
                lane: id_to_lane.get(&task.id()).copied().unwrap_or_default(),
                depth: id_to_depth.get(&task.id()).copied().unwrap_or_default(),
                ordinal: idx + 1,
                status: TaskNodeStatus::Pending,
            })
            .collect::<Vec<_>>();

        Self {
            revision: plan.revision,
            summary: plan.summary_line(),
            total: plan.tasks.len(),
            completed: 0,
            nodes,
        }
    }

    pub fn contains_task(&self, task_id: Uuid) -> bool {
        self.nodes.iter().any(|node| node.task_id == task_id)
    }

    pub fn update_task_status(&mut self, task_id: Uuid, status: TaskNodeStatus) {
        if let Some(node) = self.nodes.iter_mut().find(|node| node.task_id == task_id) {
            node.status = status;
        }
    }

    pub fn update_progress(&mut self, completed: usize, total: usize) {
        self.completed = completed.min(total);
        self.total = total.max(self.total);
    }

    fn has_running_nodes(&self) -> bool {
        self.nodes
            .iter()
            .any(|node| node.status == TaskNodeStatus::Running)
    }

    fn column_count(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.lane)
            .max()
            .map(|max| max + 1)
            .unwrap_or(0)
    }

    fn row_count(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.depth)
            .max()
            .map(|max| max + 1)
            .unwrap_or(0)
    }

    fn fallback_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self.header_lines(width);
        for node in &self.nodes {
            let status = match node.status {
                TaskNodeStatus::Pending => "○",
                TaskNodeStatus::Running => "◉",
                TaskNodeStatus::Completed => "●",
                TaskNodeStatus::Failed => "✕",
                TaskNodeStatus::Skipped => "◌",
            };
            let label = format!(
                "{} {}",
                status,
                truncate_utf8(
                    &format!("{:02} {}", node.ordinal, node_display_label(node)),
                    width.saturating_sub(4) as usize,
                )
            );
            lines.push(Line::styled(label, node_style(&node.status)));
        }
        lines.push(Line::raw(""));
        lines
    }

    fn header_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let summary = format!(
            "● DAG rev {}  {}/{}",
            self.revision, self.completed, self.total
        );
        if self.has_running_nodes() {
            lines.push(gradient_line(
                &summary,
                &theme::animation::executing_gradient(),
                animation_phase(100),
                true,
            ));
        } else {
            let color = if self
                .nodes
                .iter()
                .any(|node| node.status == TaskNodeStatus::Failed)
            {
                theme::status::danger_color()
            } else if self.completed > 0 && self.completed == self.total {
                theme::status::success_color()
            } else {
                theme::interactive::accent()
                    .fg
                    .unwrap_or(theme::text::primary().fg.unwrap_or(Color::Reset))
            };
            lines.push(Line::styled(
                summary,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
        lines.extend(wrap_text(
            &self.summary,
            "  ",
            width,
            theme::text::muted().add_modifier(Modifier::DIM),
        ));
        lines
    }
}

impl HistoryCell for DagHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self.header_lines(width);
        let lane_count = self.column_count();
        let depth_count = self.row_count();
        if lane_count == 0 || depth_count == 0 {
            lines.push(Line::raw(""));
            return lines;
        }

        let gap_width = 5usize;
        let available_width = width as usize;
        let min_node_width = 18usize;
        let node_width = available_width
            .saturating_sub(gap_width.saturating_mul(lane_count.saturating_sub(1)))
            .checked_div(lane_count)
            .unwrap_or(0)
            .min(30);
        if node_width < min_node_width {
            return self.fallback_lines(width);
        }

        let total_width =
            lane_count * node_width + gap_width.saturating_mul(lane_count.saturating_sub(1));
        let grid_height = depth_count.saturating_mul(2).saturating_sub(1).max(1);
        let mut grid = vec![vec![DagGridCell::default(); total_width]; grid_height];

        let node_positions = self
            .nodes
            .iter()
            .map(|node| {
                let x = node.lane * (node_width + gap_width);
                let y = node.depth * 2;
                (node.task_id, (x, y, x + node_width / 2))
            })
            .collect::<BTreeMap<_, _>>();

        for node in &self.nodes {
            let Some(&(_target_x, target_y, target_center)) = node_positions.get(&node.task_id)
            else {
                continue;
            };
            for dependency in &node.dependencies {
                let Some(&(_dep_x, dep_y, dep_center)) = node_positions.get(dependency) else {
                    continue;
                };
                let animated = matches!(node.status, TaskNodeStatus::Running)
                    || matches!(
                        self.nodes
                            .iter()
                            .find(|candidate| candidate.task_id == *dependency)
                            .map(|candidate| &candidate.status),
                        Some(TaskNodeStatus::Running)
                    );
                let connector_y = target_y.saturating_sub(1);
                if dep_y + 1 < connector_y {
                    draw_vertical_segment(&mut grid, dep_center, dep_y + 1, connector_y, animated);
                }
                draw_horizontal_segment(
                    &mut grid,
                    connector_y,
                    dep_center,
                    target_center,
                    animated,
                );
            }
        }

        let phase = animation_phase(110);
        let mut graph_lines = Vec::new();
        for y in 0..grid_height {
            let mut cells = Vec::with_capacity(total_width);
            for x in 0..total_width {
                let edge = &grid[y][x];
                let style = if edge.mask == 0 {
                    Style::default()
                } else if edge.animated {
                    Style::default().fg(theme::animation::executing_gradient()
                        [(x + y + phase) % theme::animation::executing_gradient().len()])
                } else {
                    theme::text::subtle().add_modifier(Modifier::DIM)
                };
                let ch = edge_glyph(edge.mask);
                cells.push((ch, style));
            }

            for node in &self.nodes {
                let Some(&(node_x, node_y, _)) = node_positions.get(&node.task_id) else {
                    continue;
                };
                if node_y != y {
                    continue;
                }
                let rendered = node_box_text(node, node_width);
                let style = node_style(&node.status);
                for (offset, ch) in rendered.chars().enumerate() {
                    if node_x + offset >= total_width {
                        break;
                    }
                    cells[node_x + offset] = (ch, style);
                }
            }

            graph_lines.push(line_from_cells(cells, "  "));
        }

        lines.extend(graph_lines);
        lines.push(Line::raw(""));
        lines
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn draw_horizontal_segment(
    grid: &mut [Vec<DagGridCell>],
    y: usize,
    start_x: usize,
    end_x: usize,
    animated: bool,
) {
    let Some(row) = grid.get_mut(y) else {
        return;
    };
    let (from, to) = if start_x <= end_x {
        (start_x, end_x)
    } else {
        (end_x, start_x)
    };
    if from == to {
        if let Some(cell) = row.get_mut(from) {
            cell.mask |= 0b1010;
            cell.animated |= animated;
        }
        return;
    }
    for x in from..=to {
        if let Some(cell) = row.get_mut(x) {
            if x > from {
                cell.mask |= 0b1000;
            }
            if x < to {
                cell.mask |= 0b0010;
            }
            cell.animated |= animated;
        }
    }
}

fn draw_vertical_segment(
    grid: &mut [Vec<DagGridCell>],
    x: usize,
    start_y: usize,
    end_y: usize,
    animated: bool,
) {
    let (from, to) = if start_y <= end_y {
        (start_y, end_y)
    } else {
        (end_y, start_y)
    };
    if from == to {
        if let Some(row) = grid.get_mut(from)
            && let Some(cell) = row.get_mut(x)
        {
            cell.mask |= 0b0101;
            cell.animated |= animated;
        }
        return;
    }
    for y in from..=to {
        let Some(row) = grid.get_mut(y) else {
            continue;
        };
        let Some(cell) = row.get_mut(x) else {
            continue;
        };
        if y > from {
            cell.mask |= 0b0001;
        }
        if y < to {
            cell.mask |= 0b0100;
        }
        cell.animated |= animated;
    }
}

fn edge_glyph(mask: u8) -> char {
    match mask {
        0 => ' ',
        0b0010 | 0b1000 | 0b1010 => '─',
        0b0001 | 0b0100 | 0b0101 => '│',
        0b0110 => '┌',
        0b1100 => '┐',
        0b0011 => '└',
        0b1001 => '┘',
        0b0111 => '├',
        0b1101 => '┤',
        0b1110 => '┬',
        0b1011 => '┴',
        0b1111 => '┼',
        _ => '•',
    }
}

fn line_from_cells(cells: Vec<(char, Style)>, prefix: &str) -> Line<'static> {
    let mut spans = vec![Span::raw(prefix.to_string())];
    let mut current_style = None;
    let mut current_text = String::new();

    for (ch, style) in cells {
        if current_style == Some(style) {
            current_text.push(ch);
            continue;
        }
        if !current_text.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut current_text),
                current_style.unwrap_or_default(),
            ));
        }
        current_style = Some(style);
        current_text.push(ch);
    }

    if !current_text.is_empty() {
        spans.push(Span::styled(
            current_text,
            current_style.unwrap_or_default(),
        ));
    }

    Line::from(spans)
}

fn node_display_label(node: &DagNodeEntry) -> String {
    let prefix = match node.kind {
        TaskKind::Implementation => "I",
        TaskKind::Gate => "G",
    };
    format!("{prefix}{:02} {}", node.ordinal, node.title)
}

fn node_box_text(node: &DagNodeEntry, width: usize) -> String {
    let inner_width = width.saturating_sub(2);
    let label = fit_label(&node_display_label(node), inner_width);
    format!("[{label}]")
}

fn fit_label(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let text = if text.chars().count() > width {
        if width <= 1 {
            "…".to_string()
        } else {
            let keep = width.saturating_sub(1);
            let mut out = text.chars().take(keep).collect::<String>();
            out.push('…');
            out
        }
    } else {
        text.to_string()
    };
    let padding = width.saturating_sub(text.chars().count());
    format!("{text}{}", " ".repeat(padding))
}

fn node_style(status: &TaskNodeStatus) -> Style {
    match status {
        TaskNodeStatus::Pending => theme::text::subtle(),
        TaskNodeStatus::Running => theme::interactive::in_progress(),
        TaskNodeStatus::Completed => theme::status::success(),
        TaskNodeStatus::Failed => theme::status::danger().add_modifier(Modifier::BOLD),
        TaskNodeStatus::Skipped => theme::text::muted().add_modifier(Modifier::DIM),
    }
}

#[derive(Debug, Clone)]
struct ExecutionTaskRow {
    label: String,
    status: TaskNodeStatus,
    retries: u8,
    tools: usize,
    violations: usize,
}

#[derive(Debug, Clone)]
struct VerificationRow {
    label: &'static str,
    passed: bool,
    detail: String,
}

#[derive(Debug, Clone)]
struct CheckpointRow {
    revision: u32,
    reason: String,
    snapshot_id: Option<String>,
    decision_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OrchestratorResultHistoryCell {
    decision: DecisionOutcome,
    revision: u32,
    task_count: usize,
    parallelism: u8,
    parallel_groups: usize,
    replan_count: u32,
    intent_id: String,
    run_id: Option<String>,
    decision_id: Option<String>,
    initial_snapshot_id: Option<String>,
    provenance_id: Option<String>,
    verification: Vec<VerificationRow>,
    review_findings: Vec<String>,
    missing_artifacts: Vec<String>,
    tasks: Vec<ExecutionTaskRow>,
    persisted_tasks: usize,
    checkpoints: Vec<CheckpointRow>,
}

impl OrchestratorResultHistoryCell {
    pub fn new(result: OrchestratorResult) -> Self {
        let task_titles: HashMap<_, _> = result
            .execution_plan_spec
            .tasks
            .iter()
            .enumerate()
            .map(|(idx, task)| {
                let prefix = match task.kind {
                    TaskKind::Implementation => "I",
                    TaskKind::Gate => "G",
                };
                (
                    task.id(),
                    format!("{prefix}{:02} {}", idx + 1, task.title()),
                )
            })
            .collect();
        let verification = vec![
            VerificationRow {
                label: "Integration",
                passed: result.system_report.integration.all_required_passed,
                detail: gate_report_detail(&result.system_report.integration),
            },
            VerificationRow {
                label: "Security",
                passed: result.system_report.security.all_required_passed,
                detail: gate_report_detail(&result.system_report.security),
            },
            VerificationRow {
                label: "Release",
                passed: result.system_report.release.all_required_passed,
                detail: gate_report_detail(&result.system_report.release),
            },
            VerificationRow {
                label: "Review",
                passed: result.system_report.review_passed,
                detail: if result.system_report.review_findings.is_empty() {
                    "no findings".to_string()
                } else {
                    format!("{} findings", result.system_report.review_findings.len())
                },
            },
            VerificationRow {
                label: "Artifacts",
                passed: result.system_report.artifacts_complete,
                detail: if result.system_report.missing_artifacts.is_empty() {
                    "complete".to_string()
                } else {
                    format!("missing {}", result.system_report.missing_artifacts.len())
                },
            },
        ];
        let task_results = result
            .task_results
            .iter()
            .map(|task| (task.task_id, task))
            .collect::<HashMap<_, _>>();
        let tasks = result
            .execution_plan_spec
            .tasks
            .iter()
            .map(|task| {
                let task_result = task_results.get(&task.id()).copied();
                ExecutionTaskRow {
                    label: task_titles
                        .get(&task.id())
                        .cloned()
                        .unwrap_or_else(|| format!("Task {}", short_uuid(&task.id()))),
                    status: task_result
                        .map(|task| task.status.clone())
                        .unwrap_or(TaskNodeStatus::Pending),
                    retries: task_result.map(|task| task.retry_count).unwrap_or_default(),
                    tools: task_result
                        .map(|task| task.tool_calls.len())
                        .unwrap_or_default(),
                    violations: task_result
                        .map(|task| task.policy_violations.len())
                        .unwrap_or_default(),
                }
            })
            .collect::<Vec<_>>();
        let (run_id, decision_id, initial_snapshot_id, provenance_id, persisted_tasks, checkpoints) =
            persistence_summary(result.persistence.as_ref());

        Self {
            decision: result.decision,
            revision: result.execution_plan_spec.revision,
            task_count: result.execution_plan_spec.tasks.len(),
            parallelism: result.execution_plan_spec.max_parallel,
            parallel_groups: result.execution_plan_spec.parallel_groups().len(),
            replan_count: result.replan_count,
            intent_id: result.intent_spec_id,
            run_id,
            decision_id,
            initial_snapshot_id,
            provenance_id,
            verification,
            review_findings: result.system_report.review_findings,
            missing_artifacts: result.system_report.missing_artifacts,
            tasks,
            persisted_tasks,
            checkpoints,
        }
    }

    fn render_header(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("● ", theme::text::primary().add_modifier(Modifier::BOLD)),
            Span::styled(
                "Execution Summary",
                theme::interactive::title().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("[{}]", decision_label(&self.decision)),
                decision_style(&self.decision).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::styled(
            "  DAG execution flow is shown in the card above.",
            theme::text::muted().add_modifier(Modifier::DIM),
        ));
        lines
    }

    fn render_overview(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = section_header("Overview");
        lines.extend(render_metric_lines(
            width,
            &[
                (
                    "rev",
                    self.revision.to_string(),
                    theme::interactive::accent(),
                ),
                ("tasks", self.task_count.to_string(), theme::text::primary()),
                (
                    "parallel",
                    self.parallelism.to_string(),
                    theme::text::primary(),
                ),
                (
                    "groups",
                    self.parallel_groups.to_string(),
                    theme::text::muted(),
                ),
                (
                    "replans",
                    self.replan_count.to_string(),
                    theme::text::muted(),
                ),
                (
                    "intent",
                    short_object_id(&self.intent_id),
                    theme::badge::workspace(),
                ),
            ],
        ));
        if let Some(run_id) = &self.run_id {
            lines.extend(render_metric_lines(
                width,
                &[
                    ("run", short_object_id(run_id), theme::badge::workspace()),
                    (
                        "decision",
                        self.decision_id
                            .as_deref()
                            .map(short_object_id)
                            .unwrap_or_else(|| "none".to_string()),
                        theme::text::muted(),
                    ),
                    (
                        "snapshot",
                        self.initial_snapshot_id
                            .as_deref()
                            .map(short_object_id)
                            .unwrap_or_else(|| "none".to_string()),
                        theme::text::muted(),
                    ),
                ],
            ));
        }
        lines
    }

    fn render_verification(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = section_header("Verification");
        for row in &self.verification {
            let status = if row.passed { "pass" } else { "fail" };
            let status_style = if row.passed {
                theme::status::success().add_modifier(Modifier::BOLD)
            } else {
                theme::status::danger().add_modifier(Modifier::BOLD)
            };
            lines.extend(wrap_text_mixed(
                &[
                    ("  ".to_string(), theme::text::primary()),
                    (format!("{:<11}", row.label), theme::text::subtle()),
                    (format!("{status:<5}"), status_style),
                    (format!(" {}", row.detail), theme::text::primary()),
                ],
                width,
            ));
        }
        if !self.review_findings.is_empty() || !self.missing_artifacts.is_empty() {
            lines.push(Line::raw(""));
        }
        for finding in &self.review_findings {
            lines.extend(wrap_text(
                &format!("review: {}", truncate_utf8(finding, 120)),
                "  ! ",
                width,
                theme::status::danger(),
            ));
        }
        for artifact in &self.missing_artifacts {
            lines.extend(wrap_text(
                &format!("missing artifact: {artifact}"),
                "  ! ",
                width,
                theme::status::warning(),
            ));
        }
        lines
    }

    fn render_tasks(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = section_header("Tasks");
        let status_width = 8usize;
        let retries_width = 3usize;
        let tools_width = 5usize;
        let violations_width = 4usize;
        let title_width = width as usize;
        if title_width >= 64 {
            let task_width = title_width
                .saturating_sub(
                    2 + status_width + retries_width + tools_width + violations_width + 9,
                )
                .max(16);
            lines.push(Line::from(vec![
                Span::styled("  ", theme::text::primary()),
                Span::styled(
                    pad_cell("Task", task_width),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", theme::text::primary()),
                Span::styled(
                    pad_cell("Status", status_width),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", theme::text::primary()),
                Span::styled(
                    pad_cell("R", retries_width),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", theme::text::primary()),
                Span::styled(
                    pad_cell("Tools", tools_width),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", theme::text::primary()),
                Span::styled(
                    pad_cell("Viol", violations_width),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::styled(
                format!(
                    "  {}",
                    "─".repeat(
                        task_width
                            + status_width
                            + retries_width
                            + tools_width
                            + violations_width
                            + 4
                    )
                ),
                theme::text::subtle(),
            ));
            for row in &self.tasks {
                lines.push(Line::from(vec![
                    Span::styled("  ", theme::text::primary()),
                    Span::styled(
                        pad_cell(&truncate_utf8(&row.label, task_width), task_width),
                        theme::text::primary(),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        pad_cell(task_status_label(&row.status), status_width),
                        task_status_style(&row.status),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        pad_cell(&row.retries.to_string(), retries_width),
                        theme::text::muted(),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        pad_cell(&row.tools.to_string(), tools_width),
                        theme::text::muted(),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        pad_cell(&row.violations.to_string(), violations_width),
                        if row.violations == 0 {
                            theme::text::muted()
                        } else {
                            theme::status::danger()
                        },
                    ),
                ]));
            }
        } else {
            for row in &self.tasks {
                lines.extend(wrap_text(
                    &format!(
                        "{} | {} | retries {} | tools {} | viol {}",
                        row.label,
                        task_status_label(&row.status),
                        row.retries,
                        row.tools,
                        row.violations
                    ),
                    "  • ",
                    width,
                    task_status_style(&row.status),
                ));
            }
        }
        lines
    }

    fn render_persistence(&self, width: u16) -> Vec<Line<'static>> {
        if self.run_id.is_none() && self.checkpoints.is_empty() && self.persisted_tasks == 0 {
            return Vec::new();
        }

        let mut lines = section_header("Persistence");
        lines.extend(render_metric_lines(
            width,
            &[
                (
                    "tasks",
                    self.persisted_tasks.to_string(),
                    theme::text::primary(),
                ),
                (
                    "ckpt",
                    self.checkpoints.len().to_string(),
                    theme::text::primary(),
                ),
                (
                    "prov",
                    self.provenance_id
                        .as_deref()
                        .map(short_object_id)
                        .unwrap_or_else(|| "none".to_string()),
                    theme::text::muted(),
                ),
            ],
        ));
        for checkpoint in &self.checkpoints {
            let detail = format!(
                "rev {} | {} | snapshot {} | decision {}",
                checkpoint.revision,
                truncate_utf8(&checkpoint.reason, 40),
                checkpoint
                    .snapshot_id
                    .as_deref()
                    .map(short_object_id)
                    .unwrap_or_else(|| "none".to_string()),
                checkpoint
                    .decision_id
                    .as_deref()
                    .map(short_object_id)
                    .unwrap_or_else(|| "none".to_string())
            );
            lines.extend(wrap_text(&detail, "  · ", width, theme::text::muted()));
        }
        lines
    }
}

impl HistoryCell for OrchestratorResultHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self.render_header();
        lines.push(Line::raw(""));
        lines.extend(self.render_overview(width));
        lines.push(Line::raw(""));
        lines.extend(self.render_verification(width));
        lines.push(Line::raw(""));
        lines.extend(self.render_tasks(width));
        let persistence = self.render_persistence(width);
        if !persistence.is_empty() {
            lines.push(Line::raw(""));
            lines.extend(persistence);
        }
        lines.push(Line::raw(""));
        lines
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn wrap_text_mixed(segments: &[(String, Style)], width: u16) -> Vec<Line<'static>> {
    let plain = segments
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<String>();
    if plain.chars().count() <= width as usize {
        return vec![Line::from(
            segments
                .iter()
                .map(|(text, style)| Span::styled(text.clone(), *style))
                .collect::<Vec<_>>(),
        )];
    }
    vec![Line::from(
        segments
            .iter()
            .map(|(text, style)| Span::styled(text.clone(), *style))
            .collect::<Vec<_>>(),
    )]
}

fn render_metric_lines(width: u16, metrics: &[(&str, String, Style)]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut spans = vec![Span::styled("  ", theme::text::primary())];
    let mut used = 2usize;
    for (idx, (label, value, value_style)) in metrics.iter().enumerate() {
        let segment_len = label.len() + value.len() + 5;
        if idx > 0 && used + segment_len > width as usize {
            lines.push(Line::from(std::mem::take(&mut spans)));
            spans.push(Span::styled("  ", theme::text::primary()));
            used = 2;
        } else if idx > 0 {
            spans.push(Span::styled("  ", theme::text::subtle()));
            used += 2;
        }
        spans.push(Span::styled(
            format!("[{label} "),
            theme::text::subtle().add_modifier(Modifier::DIM),
        ));
        spans.push(Span::styled(value.clone(), *value_style));
        spans.push(Span::styled(
            "]",
            theme::text::subtle().add_modifier(Modifier::DIM),
        ));
        used += segment_len;
    }
    if spans.len() > 1 {
        lines.push(Line::from(spans));
    }
    lines
}

fn section_header(title: &str) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled("  ", theme::text::primary()),
        Span::styled(
            title.to_string(),
            theme::interactive::accent().add_modifier(Modifier::BOLD),
        ),
    ])]
}

fn gate_report_detail(report: &GateReport) -> String {
    if report.results.is_empty() {
        return "no checks".to_string();
    }
    let passed = report.results.iter().filter(|item| item.passed).count();
    format!("{passed}/{} checks", report.results.len())
}

fn short_object_id(id: &str) -> String {
    if id.len() <= 12 {
        id.to_string()
    } else {
        format!("{}…", &id[..12])
    }
}

fn short_uuid(id: &Uuid) -> String {
    short_object_id(&id.to_string())
}

fn decision_label(decision: &DecisionOutcome) -> &'static str {
    match decision {
        DecisionOutcome::Commit => "Commit",
        DecisionOutcome::HumanReviewRequired => "Review",
        DecisionOutcome::Abandon => "Abandon",
    }
}

fn decision_style(decision: &DecisionOutcome) -> Style {
    match decision {
        DecisionOutcome::Commit => theme::status::success(),
        DecisionOutcome::HumanReviewRequired => theme::status::warning(),
        DecisionOutcome::Abandon => theme::status::danger(),
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

fn task_status_style(status: &TaskNodeStatus) -> Style {
    match status {
        TaskNodeStatus::Pending => theme::text::subtle(),
        TaskNodeStatus::Running => theme::interactive::in_progress(),
        TaskNodeStatus::Completed => theme::status::success(),
        TaskNodeStatus::Failed => theme::status::danger(),
        TaskNodeStatus::Skipped => theme::text::muted(),
    }
}

fn pad_cell(text: &str, width: usize) -> String {
    let truncated = truncate_utf8(text, width);
    let pad = width.saturating_sub(truncated.chars().count());
    format!("{truncated}{}", " ".repeat(pad))
}

fn persistence_summary(
    persistence: Option<&PersistedExecution>,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    usize,
    Vec<CheckpointRow>,
) {
    let Some(persistence) = persistence else {
        return (None, None, None, None, 0, Vec::new());
    };
    let checkpoints = persistence
        .checkpoints
        .iter()
        .map(|checkpoint| CheckpointRow {
            revision: checkpoint.revision,
            reason: checkpoint.reason.clone(),
            snapshot_id: checkpoint.snapshot_id.clone(),
            decision_id: checkpoint.decision_id.clone(),
        })
        .collect::<Vec<_>>();
    (
        Some(persistence.run_id.clone()),
        persistence.decision_id.clone(),
        persistence.initial_snapshot_id.clone(),
        persistence.provenance_id.clone(),
        persistence.tasks.len(),
        checkpoints,
    )
}

#[cfg(test)]
mod tests {
    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        AssistantHistoryCell, DagHistoryCell, HistoryCell, OrchestratorResultHistoryCell,
        PlanUpdateHistoryCell, ToolCallHistoryCell, UserHistoryCell,
    };
    use crate::internal::ai::{
        orchestrator::types::{
            DecisionOutcome, ExecutionPlanSpec, OrchestratorResult, SystemReport, TaskContract,
            TaskKind, TaskNodeStatus, TaskResult, TaskSpec,
        },
        tools::{
            ToolOutput,
            context::{PlanStep, StepStatus},
        },
    };

    fn to_strings(lines: Vec<ratatui::text::Line<'static>>) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    fn make_task(title: &str, kind: TaskKind, dependencies: Vec<Uuid>) -> TaskSpec {
        let actor = ActorRef::agent("dag-history-cell").unwrap();
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

    fn dag_plan() -> ExecutionPlanSpec {
        let first = make_task("Inspect sources", TaskKind::Implementation, vec![]);
        let second = make_task("Render DAG", TaskKind::Implementation, vec![]);
        let final_task = make_task("Run checks", TaskKind::Gate, vec![first.id(), second.id()]);
        ExecutionPlanSpec {
            intent_spec_id: "intent-1".into(),
            revision: 2,
            parent_revision: Some(1),
            replan_reason: None,
            tasks: vec![first, second, final_task],
            max_parallel: 2,
            checkpoints: vec![],
        }
    }

    fn serial_dag_plan() -> ExecutionPlanSpec {
        let first = make_task("Inventory todos", TaskKind::Implementation, vec![]);
        let second = make_task(
            "Assess test coverage",
            TaskKind::Implementation,
            vec![first.id()],
        );
        let third = make_task("Summarize findings", TaskKind::Gate, vec![second.id()]);
        ExecutionPlanSpec {
            intent_spec_id: "intent-serial".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![first, second, third],
            max_parallel: 1,
            checkpoints: vec![],
        }
    }

    fn orchestrator_result_fixture() -> OrchestratorResult {
        let plan = dag_plan();
        OrchestratorResult {
            decision: DecisionOutcome::Abandon,
            execution_plan_spec: plan.clone(),
            plan_revision_specs: vec![plan.clone()],
            run_state: Default::default(),
            task_results: vec![
                TaskResult {
                    task_id: plan.tasks[0].id(),
                    status: TaskNodeStatus::Completed,
                    gate_report: None,
                    agent_output: None,
                    retry_count: 0,
                    tool_calls: vec![],
                    policy_violations: vec![],
                    review: None,
                },
                TaskResult {
                    task_id: plan.tasks[1].id(),
                    status: TaskNodeStatus::Failed,
                    gate_report: None,
                    agent_output: None,
                    retry_count: 2,
                    tool_calls: vec![],
                    policy_violations: vec![],
                    review: None,
                },
            ],
            system_report: SystemReport {
                integration: crate::internal::ai::orchestrator::types::GateReport::empty(),
                security: crate::internal::ai::orchestrator::types::GateReport::empty(),
                release: crate::internal::ai::orchestrator::types::GateReport::empty(),
                review_passed: false,
                review_findings: vec!["missing regression test".into()],
                artifacts_complete: false,
                missing_artifacts: vec!["patchset@per-task".into()],
                overall_passed: false,
            },
            intent_spec_id: "019ce515-077c-7c12-8e90-755533e512e3".into(),
            lifecycle_change_log: vec![],
            replan_count: 1,
            persistence: None,
        }
    }

    #[test]
    fn user_cell_uses_vertical_bar_and_no_user_label() {
        let cell = UserHistoryCell::new("hello".to_string());
        let rendered = to_strings(cell.display_lines(80));
        assert!(rendered.iter().any(|line| line.starts_with("│ ")));
        assert!(!rendered.iter().any(|line| line.contains("User:")));
    }

    #[test]
    fn assistant_cell_uses_bullet_and_no_assistant_label() {
        let cell = AssistantHistoryCell::new("response".to_string());
        let rendered = to_strings(cell.display_lines(80));
        assert!(rendered.iter().any(|line| line.starts_with("● ")));
        assert!(!rendered.iter().any(|line| line.contains("Assistant:")));
    }

    #[test]
    fn dag_cell_renders_dependency_graph() {
        let plan = dag_plan();
        let mut cell = DagHistoryCell::new(plan.clone());
        cell.update_task_status(plan.tasks[0].id(), TaskNodeStatus::Completed);
        cell.update_task_status(plan.tasks[1].id(), TaskNodeStatus::Running);
        cell.update_progress(1, plan.tasks.len());

        let rendered = to_strings(cell.display_lines(120));
        assert!(rendered.iter().any(|line| line.contains("DAG rev 2")));
        assert!(rendered.iter().any(|line| line.contains("Inspect sources")));
        assert!(rendered.iter().any(|line| line.contains("Render DAG")));
        assert!(rendered.iter().any(|line| line.contains("Run checks")));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains('─') || line.contains('│'))
        );
    }

    #[test]
    fn dag_cell_falls_back_on_narrow_width() {
        let cell = DagHistoryCell::new(dag_plan());
        let rendered = to_strings(cell.display_lines(28));
        assert!(rendered.iter().any(|line| line.contains("○")));
        assert!(!rendered.iter().any(|line| line.contains("[impl")));
    }

    #[test]
    fn dag_cell_keeps_serial_plans_vertical() {
        let plan = serial_dag_plan();
        let cell = DagHistoryCell::new(plan);
        let rendered = to_strings(cell.display_lines(100));
        let joined = rendered.join("\n");

        assert!(joined.contains("Inventory todos"));
        assert!(joined.contains("Assess test coverage"));
        assert!(joined.contains("Summarize findings"));
        assert!(!rendered.iter().any(|line| line.starts_with("○   ○")));
    }

    #[test]
    fn orchestrator_result_cell_renders_structured_sections() {
        let cell = OrchestratorResultHistoryCell::new(orchestrator_result_fixture());
        let rendered = to_strings(cell.display_lines(100));
        let joined = rendered.join("\n");

        assert!(joined.contains("Execution Summary"));
        assert!(joined.contains("[Abandon]"));
        assert!(joined.contains("Overview"));
        assert!(joined.contains("Verification"));
        assert!(joined.contains("Tasks"));
        assert!(joined.contains("I01 Inspect sources"));
        assert!(joined.contains("I02 Render DAG"));
        assert!(joined.contains("missing artifact"));
    }

    #[test]
    fn streaming_placeholder_does_not_render_standalone_cursor_line() {
        let cell = AssistantHistoryCell::streaming();
        let rendered = to_strings(cell.display_lines(80));
        assert!(!rendered.iter().any(|line| line.contains("Thinking")));
        assert!(rendered.is_empty());
        assert!(!rendered.iter().any(|line| line.trim() == "▌"));
    }

    #[test]
    fn tool_cell_header_uses_bullet() {
        let cell = ToolCallHistoryCell::new("1".to_string(), "read_file".to_string(), json!({}));
        let rendered = to_strings(cell.display_lines(80));
        assert!(rendered.iter().any(|line| line.starts_with("● ")));
    }

    #[test]
    fn tool_cell_hides_raw_args_and_results() {
        let mut cell = ToolCallHistoryCell::new(
            "1".to_string(),
            "read_file".to_string(),
            json!({"file_path":"src/main.rs"}),
        );
        cell.complete_call("1", Ok(ToolOutput::success("L1: fn main() {}")));

        let rendered = to_strings(cell.display_lines(80));
        let joined = rendered.join("\n");

        assert!(joined.contains("Explored"));
        assert!(joined.contains("Read src/main.rs"));
        assert!(!joined.contains("Args:"));
        assert!(!joined.contains("Result:"));
        assert!(!joined.contains("L1: fn main() {}"));
    }

    #[test]
    fn tool_cell_renders_grouped_entries() {
        let mut cell = ToolCallHistoryCell::new(
            "1".to_string(),
            "grep_files".to_string(),
            json!({"pattern":"cwd|pwd","path":"src"}),
        );
        cell.append_call(
            "2".to_string(),
            "list_dir".to_string(),
            json!({"dir_path":"src"}),
        );
        cell.complete_call("1", Ok(ToolOutput::success("src/internal/tui/app.rs")));
        cell.complete_call("2", Ok(ToolOutput::success("Absolute path: /tmp/src")));

        let rendered = to_strings(cell.display_lines(100));
        let joined = rendered.join("\n");

        assert!(joined.contains("Explored"));
        assert!(joined.contains("Search cwd|pwd in src"));
        assert!(joined.contains("List src"));
    }

    #[test]
    fn plan_cell_header_uses_bullet() {
        let cell = PlanUpdateHistoryCell::new(
            "1".to_string(),
            None,
            vec![PlanStep {
                step: "do work".to_string(),
                status: StepStatus::InProgress,
            }],
        );
        let rendered = to_strings(cell.display_lines(80));
        assert!(rendered.iter().any(|line| line.starts_with("● ")));
    }
}
