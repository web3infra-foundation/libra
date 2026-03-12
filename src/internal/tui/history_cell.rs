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

use super::diff::{DiffSummary, FileChange, create_diff_summary};
use crate::internal::ai::tools::{
    ToolOutput,
    context::{PlanStep, StepStatus},
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

const ACTIVE_GRADIENT_COLORS: [Color; 5] = [
    Color::Rgb(76, 108, 152),
    Color::Rgb(84, 124, 160),
    Color::Rgb(156, 168, 188),
    Color::Rgb(84, 124, 160),
    Color::Rgb(76, 108, 152),
];

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
            lines.extend(wrap_text(
                line,
                "│ ",
                width,
                Style::default().fg(Color::Cyan),
            ));
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

        // Simple markdown-like rendering
        let content = self.content.trim();
        if !content.is_empty() {
            for (idx, line) in content.lines().enumerate() {
                let prefix = if idx == 0 { "● " } else { "  " };
                // Simple code block detection
                if line.starts_with("```") {
                    lines.extend(wrap_text(
                        line,
                        prefix,
                        width,
                        Style::default().fg(Color::Yellow),
                    ));
                } else if line.starts_with("    ") || line.starts_with("\t") {
                    // Code indent
                    lines.extend(wrap_text(
                        line,
                        prefix,
                        width,
                        Style::default().fg(Color::Yellow),
                    ));
                } else {
                    lines.extend(wrap_text(line, prefix, width, Style::default()));
                }
            }
        }

        if self.is_streaming && !content.is_empty() {
            lines.push(Line::styled("  ▌", Style::default().fg(Color::Green)));
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
            Self::Explore => Style::default().fg(Color::Rgb(128, 154, 194)),
            Self::Edit => Style::default().fg(Color::Rgb(176, 156, 98)),
            Self::Shell => Style::default().fg(Color::Rgb(102, 146, 102)),
            Self::Input => Style::default().fg(Color::Rgb(152, 124, 152)),
            Self::Draft => Style::default().fg(Color::Rgb(98, 146, 152)),
            Self::Other(_) => Style::default().fg(Color::DarkGray),
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
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.call_id == call_id) {
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
                &ACTIVE_GRADIENT_COLORS,
                phase,
                true,
            ));
        } else {
            let status_color = if self.is_success() {
                Color::Rgb(96, 136, 96)
            } else {
                Color::Rgb(148, 102, 102)
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
                    Style::default().fg(Color::Red).add_modifier(Modifier::DIM),
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
        "read_file" => format!("Read {}", argument_string(arguments, "file_path").unwrap_or("?")),
        "list_dir" => format!("List {}", argument_string(arguments, "dir_path").unwrap_or(".")),
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
            truncate_utf8(argument_string(arguments, "command").unwrap_or("(command)"), 120)
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

fn wrap_tool_entry(summary: &str, prefix: &str, width: u16, action_style: Style) -> Vec<Line<'static>> {
    let (action, detail) = summary
        .split_once(' ')
        .map_or((summary, ""), |(action, detail)| (action, detail));

    if detail.is_empty() {
        return vec![Line::from(vec![
            Span::styled(prefix.to_string(), Style::default().fg(Color::White)),
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
            Span::styled(prefix.to_string(), Style::default().fg(Color::White)),
            Span::styled(action.to_string(), action_style),
            Span::styled(format!(" {first}"), Style::default().fg(Color::White)),
        ]));

        for chunk in rest {
            lines.push(Line::from(vec![Span::styled(
                format!("{continuation_prefix}{chunk}"),
                Style::default().fg(Color::White),
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
        let mut lines = vec![Line::styled(
            "● Diff",
            Style::default().fg(Color::White).bold(),
        )];
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
            Color::Yellow
        } else {
            Color::Green
        };
        lines.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::White).bold()),
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
                StepStatus::InProgress => ("◐", Style::default().fg(Color::Cyan).bold()),
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AssistantHistoryCell, HistoryCell, PlanUpdateHistoryCell, ToolCallHistoryCell,
        UserHistoryCell,
    };
    use crate::internal::ai::tools::{
        ToolOutput,
        context::{PlanStep, StepStatus},
    };

    fn to_strings(lines: Vec<ratatui::text::Line<'static>>) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
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
