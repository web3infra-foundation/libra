//! History cells for the chat transcript.
//!
//! A `HistoryCell` is the unit of display in the conversation UI, representing
//! user messages, assistant responses, and tool calls.

use std::{
    any::Any,
    collections::HashMap,
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
    intentspec::types::IntentSpec,
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

    fn hide_failed_calls(&self) -> bool {
        matches!(self, Self::Explore | Self::Edit)
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

#[derive(Debug)]
struct ToolEntryRun<'a> {
    action: &'a str,
    details: Vec<&'a str>,
    failures: Vec<&'a str>,
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

    pub fn remove_call(&mut self, call_id: &str) {
        self.entries.retain(|entry| entry.call_id != call_id);
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn hides_failed_calls(&self) -> bool {
        self.group.hide_failed_calls()
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

        let grouped_entries = group_tool_entries(&self.entries);
        for (idx, entry) in grouped_entries.iter().enumerate() {
            let prefix = if idx + 1 == grouped_entries.len() {
                "  └ "
            } else {
                "  ├ "
            };
            let summary = if entry.details.is_empty() {
                entry.action.to_string()
            } else {
                format!("{} {}", entry.action, entry.details.join("  •  "))
            };
            lines.extend(wrap_tool_entry(
                &summary,
                prefix,
                width,
                self.group.action_style(),
            ));

            for error in &entry.failures {
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

fn group_tool_entries(entries: &[ToolCallEntry]) -> Vec<ToolEntryRun<'_>> {
    let mut grouped: Vec<ToolEntryRun<'_>> = Vec::new();

    for entry in entries {
        let (action, detail) = split_tool_summary(&entry.summary);
        let failure = match &entry.status {
            ToolCallEntryStatus::Failed(error) => Some(error.as_str()),
            ToolCallEntryStatus::Running | ToolCallEntryStatus::Success => None,
        };

        if let Some(last) = grouped.last_mut()
            && last.action == action
        {
            if !detail.is_empty() {
                last.details.push(detail);
            }
            if let Some(error) = failure {
                last.failures.push(error);
            }
            continue;
        }

        grouped.push(ToolEntryRun {
            action,
            details: if detail.is_empty() {
                Vec::new()
            } else {
                vec![detail]
            },
            failures: failure.into_iter().collect(),
        });
    }

    grouped
}

fn split_tool_summary(summary: &str) -> (&str, &str) {
    summary
        .split_once(' ')
        .map_or((summary, ""), |(action, detail)| (action, detail))
}

fn wrap_tool_entry(
    summary: &str,
    prefix: &str,
    width: u16,
    action_style: Style,
) -> Vec<Line<'static>> {
    let (action, detail) = split_tool_summary(summary);

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
struct PlanTaskRow {
    label: String,
    kind: String,
    dependencies: usize,
    files: usize,
    checks: usize,
}

#[derive(Debug, Clone)]
pub struct PlanSummaryHistoryCell {
    summary: String,
    risk: String,
    change_type: String,
    objective_count: usize,
    verification_count: usize,
    artifact_count: usize,
    intent_id: Option<String>,
    plan_id: Option<String>,
    max_parallelism: u8,
    lane_count: usize,
    layer_count: usize,
    checkpoint_count: usize,
    tasks: Vec<PlanTaskRow>,
    warnings: Vec<String>,
}

impl PlanSummaryHistoryCell {
    pub fn new(
        spec: IntentSpec,
        plan: ExecutionPlanSpec,
        intent_id: Option<String>,
        plan_id: Option<String>,
        warnings: Vec<String>,
    ) -> Self {
        let (lane_count, layer_count) = parallel_layout_stats(&plan);
        let verification_count = spec.acceptance.verification_plan.fast_checks.len()
            + spec.acceptance.verification_plan.integration_checks.len()
            + spec.acceptance.verification_plan.security_checks.len()
            + spec.acceptance.verification_plan.release_checks.len();
        let tasks = plan
            .tasks
            .iter()
            .enumerate()
            .map(|(idx, task)| PlanTaskRow {
                label: format!(
                    "{}{:02} {}",
                    match task.kind {
                        TaskKind::Implementation => "I",
                        TaskKind::Analysis => "A",
                        TaskKind::Gate => "G",
                    },
                    idx + 1,
                    task.title()
                ),
                kind: match task.gate_stage.as_ref() {
                    Some(stage) => format!("{stage:?}").to_lowercase(),
                    None => match task.kind {
                        TaskKind::Implementation => "impl".to_string(),
                        TaskKind::Analysis => "analysis".to_string(),
                        TaskKind::Gate => "gate".to_string(),
                    },
                },
                dependencies: task.dependencies().len(),
                files: task.contract.touch_files.len(),
                checks: task.checks.len(),
            })
            .collect::<Vec<_>>();

        Self {
            summary: spec.intent.summary,
            risk: format!("{:?}", spec.risk.level),
            change_type: format!("{:?}", spec.intent.change_type).to_lowercase(),
            objective_count: spec.intent.objectives.len(),
            verification_count,
            artifact_count: spec.artifacts.required.len(),
            intent_id,
            plan_id,
            max_parallelism: plan.max_parallel,
            lane_count,
            layer_count,
            checkpoint_count: plan.checkpoints.len(),
            tasks,
            warnings,
        }
    }

    fn render_header(&self) -> Vec<Line<'static>> {
        vec![
            Line::from(vec![
                Span::styled("● ", theme::text::primary().add_modifier(Modifier::BOLD)),
                Span::styled(
                    "Plan Ready",
                    theme::interactive::title().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::styled(
                "  Workflow graph is shown in the side panel.",
                theme::text::muted().add_modifier(Modifier::DIM),
            ),
        ]
    }

    fn render_overview(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = section_header("Overview");
        lines.extend(wrap_text(
            &self.summary,
            "  ",
            width,
            theme::text::primary().add_modifier(Modifier::BOLD),
        ));
        lines.extend(render_metric_lines(
            width,
            &[
                ("risk", self.risk.clone(), theme::status::warning()),
                ("type", self.change_type.clone(), theme::text::primary()),
                (
                    "goals",
                    self.objective_count.to_string(),
                    theme::text::primary(),
                ),
                (
                    "checks",
                    self.verification_count.to_string(),
                    theme::text::primary(),
                ),
                (
                    "artifacts",
                    self.artifact_count.to_string(),
                    theme::text::primary(),
                ),
            ],
        ));
        lines.extend(render_metric_lines(
            width,
            &[
                (
                    "max",
                    self.max_parallelism.to_string(),
                    theme::text::primary(),
                ),
                ("lanes", self.lane_count.to_string(), theme::text::primary()),
                ("layers", self.layer_count.to_string(), theme::text::muted()),
                (
                    "ckpt",
                    self.checkpoint_count.to_string(),
                    theme::text::muted(),
                ),
                (
                    "intent",
                    self.intent_id
                        .as_deref()
                        .map(short_object_id)
                        .unwrap_or_else(|| "none".to_string()),
                    theme::badge::workspace(),
                ),
                (
                    "plan",
                    self.plan_id
                        .as_deref()
                        .map(short_object_id)
                        .unwrap_or_else(|| "none".to_string()),
                    theme::badge::workspace(),
                ),
            ],
        ));
        lines
    }

    fn render_tasks(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = section_header("Tasks");
        let label_width = width.saturating_sub(24) as usize;
        if width >= 68 {
            lines.push(Line::from(vec![
                Span::styled("  ", theme::text::primary()),
                Span::styled(
                    pad_cell("Task", label_width.max(18)),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    pad_cell("Type", 9),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    pad_cell("Deps", 4),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    pad_cell("Files", 5),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    pad_cell("Checks", 6),
                    theme::text::subtle().add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::styled(
                format!("  {}", "─".repeat(width.saturating_sub(4) as usize)),
                theme::text::subtle(),
            ));
            for row in &self.tasks {
                lines.push(Line::from(vec![
                    Span::styled("  ", theme::text::primary()),
                    Span::styled(
                        pad_cell(
                            &truncate_utf8(&row.label, label_width.max(18)),
                            label_width.max(18),
                        ),
                        theme::text::primary(),
                    ),
                    Span::raw(" "),
                    Span::styled(pad_cell(&row.kind, 9), theme::text::muted()),
                    Span::raw(" "),
                    Span::styled(
                        pad_cell(&row.dependencies.to_string(), 4),
                        theme::text::muted(),
                    ),
                    Span::raw(" "),
                    Span::styled(pad_cell(&row.files.to_string(), 5), theme::text::muted()),
                    Span::raw(" "),
                    Span::styled(pad_cell(&row.checks.to_string(), 6), theme::text::muted()),
                ]));
            }
        } else {
            for row in &self.tasks {
                lines.extend(wrap_text(
                    &format!(
                        "{} | {} | deps {} | files {} | checks {}",
                        row.label, row.kind, row.dependencies, row.files, row.checks
                    ),
                    "  • ",
                    width,
                    theme::text::primary(),
                ));
            }
        }
        lines
    }

    fn render_warnings(&self, width: u16) -> Vec<Line<'static>> {
        if self.warnings.is_empty() {
            return Vec::new();
        }
        let mut lines = section_header("Warnings");
        for warning in &self.warnings {
            lines.extend(wrap_text(warning, "  ! ", width, theme::status::warning()));
        }
        lines
    }
}

impl HistoryCell for PlanSummaryHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = self.render_header();
        lines.push(Line::raw(""));
        lines.extend(self.render_overview(width));
        lines.push(Line::raw(""));
        lines.extend(self.render_tasks(width));
        let warnings = self.render_warnings(width);
        if !warnings.is_empty() {
            lines.push(Line::raw(""));
            lines.extend(warnings);
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
    max_parallelism: u8,
    lane_count: usize,
    layer_count: usize,
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
        let (lane_count, layer_count) = parallel_layout_stats(&result.execution_plan_spec);
        let task_titles: HashMap<_, _> = result
            .execution_plan_spec
            .tasks
            .iter()
            .enumerate()
            .map(|(idx, task)| {
                let prefix = match task.kind {
                    TaskKind::Implementation => "I",
                    TaskKind::Analysis => "A",
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
            max_parallelism: result.execution_plan_spec.max_parallel,
            lane_count,
            layer_count,
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
                    "max",
                    self.max_parallelism.to_string(),
                    theme::text::primary(),
                ),
                ("lanes", self.lane_count.to_string(), theme::text::primary()),
                ("layers", self.layer_count.to_string(), theme::text::muted()),
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

fn parallel_layout_stats(plan: &ExecutionPlanSpec) -> (usize, usize) {
    let groups = plan.parallel_groups();
    let lane_count = groups.iter().map(Vec::len).max().unwrap_or(0);
    let layer_count = groups.len();
    (lane_count, layer_count)
}

fn pad_cell(text: &str, width: usize) -> String {
    let truncated = truncate_utf8(text, width);
    let pad = width.saturating_sub(truncated.chars().count());
    format!("{truncated}{}", " ".repeat(pad))
}

type PersistenceSummary = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    usize,
    Vec<CheckpointRow>,
);

fn persistence_summary(persistence: Option<&PersistedExecution>) -> PersistenceSummary {
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
        AssistantHistoryCell, HistoryCell, OrchestratorResultHistoryCell, PlanSummaryHistoryCell,
        PlanUpdateHistoryCell, ToolCallHistoryCell, UserHistoryCell,
    };
    use crate::internal::ai::{
        intentspec::{
            ResolveContext,
            draft::{DraftAcceptance, DraftIntent, DraftRisk, IntentDraft},
            resolve_intentspec,
            types::{ChangeType, IntentSpec, Objective, ObjectiveKind, RiskLevel},
        },
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

    fn intentspec_fixture() -> IntentSpec {
        resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "Refine TUI workflow presentation".to_string(),
                    problem_statement: "Plan summary is too verbose".to_string(),
                    change_type: ChangeType::Refactor,
                    objectives: vec![
                        Objective {
                            title: "compact plan summary".to_string(),
                            kind: ObjectiveKind::Analysis,
                        },
                        Objective {
                            title: "show dag in side panel".to_string(),
                            kind: ObjectiveKind::Analysis,
                        },
                    ],
                    in_scope: vec!["src/internal/tui".to_string()],
                    out_of_scope: vec![],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["ui is readable".to_string()],
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                risk: DraftRisk {
                    rationale: "ui-only".to_string(),
                    factors: vec![],
                    level: Some(RiskLevel::Low),
                },
            },
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        )
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
    fn plan_summary_cell_renders_compact_sections() {
        let cell = PlanSummaryHistoryCell::new(
            intentspec_fixture(),
            dag_plan(),
            Some("019ce52e-18c8-7530-9472-92599ad7bcd0".to_string()),
            Some("019ce52e-18d5-7910-b999-a6782e91666e".to_string()),
            vec!["execution plan not persisted".to_string()],
        );
        let rendered = to_strings(cell.display_lines(100));
        let joined = rendered.join("\n");

        assert!(joined.contains("Plan Ready"));
        assert!(joined.contains("Overview"));
        assert!(joined.contains("Tasks"));
        assert!(joined.contains("Warnings"));
        assert!(joined.contains("I01 Inspect sources"));
        assert!(joined.contains("[max 2]"));
        assert!(joined.contains("[lanes 2]"));
        assert!(joined.contains("[layers 2]"));
        assert!(!joined.contains("files:"));
        assert!(!joined.contains("depends on:"));
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
        assert!(joined.contains("[max 2]"));
        assert!(joined.contains("[lanes 2]"));
        assert!(joined.contains("[layers 2]"));
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
    fn tool_cell_compacts_consecutive_same_action_into_single_line() {
        let mut cell = ToolCallHistoryCell::new(
            "1".to_string(),
            "read_file".to_string(),
            json!({"file_path":"README.md"}),
        );
        cell.append_call(
            "2".to_string(),
            "read_file".to_string(),
            json!({"file_path":"CLAUDE.md"}),
        );
        cell.append_call(
            "3".to_string(),
            "read_file".to_string(),
            json!({"file_path":"Cargo.toml"}),
        );
        cell.complete_call("1", Ok(ToolOutput::success("ok")));
        cell.complete_call("2", Ok(ToolOutput::success("ok")));
        cell.complete_call("3", Ok(ToolOutput::success("ok")));

        let rendered = to_strings(cell.display_lines(100));
        let read_lines = rendered
            .iter()
            .filter(|line| line.contains("Read "))
            .collect::<Vec<_>>();

        assert_eq!(read_lines.len(), 1);
        assert!(read_lines[0].contains("README.md"));
        assert!(read_lines[0].contains("CLAUDE.md"));
        assert!(read_lines[0].contains("Cargo.toml"));
    }

    #[test]
    fn explore_cell_can_drop_failed_call_entries() {
        let mut cell = ToolCallHistoryCell::new(
            "1".to_string(),
            "read_file".to_string(),
            json!({"file_path":"CONTRIBUTING.md"}),
        );

        assert!(cell.hides_failed_calls());
        cell.remove_call("1");
        assert!(cell.is_empty());
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
