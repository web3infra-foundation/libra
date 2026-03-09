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
    Color::Rgb(110, 160, 255),
    Color::Rgb(120, 210, 255),
    Color::Rgb(235, 245, 255),
    Color::Rgb(120, 210, 255),
    Color::Rgb(110, 160, 255),
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
#[derive(Debug, Clone)]
pub struct ToolCallHistoryCell {
    /// Unique id for this tool call.
    pub call_id: String,
    /// The name of the tool.
    pub tool_name: String,
    /// The arguments passed to the tool.
    pub arguments: Value,
    /// The result of the tool call, if complete.
    pub result: Option<Result<ToolOutput, String>>,
    /// Whether the tool is still running.
    pub is_running: bool,
}

impl ToolCallHistoryCell {
    /// Create a new tool call cell.
    pub fn new(call_id: String, tool_name: String, arguments: Value) -> Self {
        Self {
            call_id,
            tool_name,
            arguments,
            result: None,
            is_running: true,
        }
    }

    /// Complete the tool call with a result.
    pub fn complete(&mut self, result: Result<ToolOutput, String>) {
        self.result = Some(result);
        self.is_running = false;
    }

    /// Check if the tool call succeeded.
    pub fn is_success(&self) -> bool {
        self.result.as_ref().is_some_and(|r| r.is_ok())
    }
}

impl HistoryCell for ToolCallHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let phase = animation_phase(120);
        let (running_label, done_label, failed_label) = friendly_tool_labels(&self.tool_name);

        // Tool state summary line
        let summary = if self.is_running {
            running_label
        } else if self.is_success() {
            done_label
        } else {
            failed_label
        };
        if self.is_running {
            lines.push(gradient_line(
                &format!("● {summary}"),
                &ACTIVE_GRADIENT_COLORS,
                phase,
                true,
            ));
        } else {
            let status_color = if self.is_success() {
                Color::Green
            } else {
                Color::Red
            };
            lines.push(Line::styled(
                format!("● {summary}"),
                Style::default().fg(status_color).bold(),
            ));
        }

        // Arguments (abbreviated)
        let args_str = self.arguments.to_string();
        let truncated = if args_str.len() > 100 {
            format!("{}...", truncate_utf8(&args_str, 100))
        } else {
            args_str
        };
        lines.push(Line::styled(
            format!("  Args: {}", truncated),
            Style::default().add_modifier(Modifier::DIM),
        ));

        // Result
        if let Some(ref result) = self.result {
            match result {
                Ok(output) => {
                    let result_str = match output {
                        ToolOutput::Function {
                            content, success, ..
                        } => {
                            let status = success
                                .map(|s| if s { "success" } else { "failed" })
                                .unwrap_or("done");
                            let preview = if content.len() > 50 {
                                format!("{}...", truncate_utf8(content, 50))
                            } else {
                                content.clone()
                            };
                            format!("{}: {}", status, preview)
                        }
                        ToolOutput::Mcp { result } => {
                            format!("MCP result: {:?}", result)
                        }
                    };
                    lines.push(Line::styled(
                        format!("  Result: {}", result_str),
                        Style::default().fg(Color::Green),
                    ));
                }
                Err(e) => {
                    // Wrap error messages so they don't overflow the terminal width.
                    lines.extend(wrap_text(
                        e,
                        "  Error: ",
                        width,
                        Style::default().fg(Color::Red),
                    ));
                }
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

fn friendly_tool_labels(tool_name: &str) -> (&'static str, &'static str, &'static str) {
    match tool_name {
        "read_file" | "list_dir" | "grep_files" => ("Exploring", "Explored", "Explore failed"),
        "apply_patch" => ("Editing", "Edited", "Edit failed"),
        "shell" => ("Running command", "Command completed", "Command failed"),
        "request_user_input" => ("Waiting for input", "Input received", "Input cancelled"),
        "update_plan" => ("Planning", "Planned", "Plan update failed"),
        "submit_intent_draft" => ("Drafting", "Drafted", "Draft failed"),
        _ => ("Working", "Completed", "Failed"),
    }
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
    use crate::internal::ai::tools::context::{PlanStep, StepStatus};

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
