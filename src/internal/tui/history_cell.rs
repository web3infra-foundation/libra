//! History cells for the chat transcript.
//!
//! A `HistoryCell` is the unit of display in the conversation UI, representing
//! user messages, assistant responses, and tool calls.

use std::{any::Any, collections::HashMap, fmt::Debug, path::PathBuf};

use ratatui::prelude::*;
use serde_json::Value;

use super::diff::{DiffSummary, FileChange, create_diff_summary};
use crate::internal::ai::tools::ToolOutput;

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
        let mut lines = vec![Line::styled(
            "User:",
            Style::default().fg(Color::Cyan).bold(),
        )];

        for line in self.message.lines() {
            lines.extend(wrap_text(line, "  ", width, Style::default().fg(Color::White)));
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

    /// Append text to the content.
    pub fn append(&mut self, text: &str) {
        self.content.push_str(text);
    }

    /// Mark the response as complete.
    pub fn complete(&mut self) {
        self.is_streaming = false;
    }
}

impl HistoryCell for AssistantHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::styled(
            "Assistant:",
            Style::default().fg(Color::Green).bold(),
        )];

        // Simple markdown-like rendering
        let content = self.content.trim();
        if content.is_empty() {
            if self.is_streaming {
                lines.push(Line::styled(
                    "  Thinking...",
                    Style::default().fg(Color::DarkGray),
                ));
            }
        } else {
            for line in content.lines() {
                // Simple code block detection
                if line.starts_with("```") {
                    lines.extend(wrap_text(
                        line,
                        "",
                        width,
                        Style::default().fg(Color::Yellow),
                    ));
                } else if line.starts_with("    ") || line.starts_with("\t") {
                    // Code indent
                    lines.extend(wrap_text(
                        line,
                        "  ",
                        width,
                        Style::default().fg(Color::Yellow),
                    ));
                } else {
                    lines.extend(wrap_text(
                        line,
                        "  ",
                        width,
                        Style::default().fg(Color::White),
                    ));
                }
            }
        }

        if self.is_streaming {
            lines.push(Line::styled("  ▌", Style::default().fg(Color::Green)));
        } else {
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

        // Status icon
        let status_icon = if self.is_running {
            "⏳"
        } else if self.is_success() {
            "✓"
        } else {
            "✗"
        };

        let status_color = if self.is_running {
            Color::Yellow
        } else if self.is_success() {
            Color::Green
        } else {
            Color::Red
        };

        // Tool name line
        lines.push(Line::styled(
            format!("{} Tool: {}", status_icon, self.tool_name),
            Style::default().fg(status_color).bold(),
        ));

        // Arguments (abbreviated)
        let args_str = self.arguments.to_string();
        let truncated = if args_str.len() > 100 {
            format!("{}...", truncate_utf8(&args_str, 100))
        } else {
            args_str
        };
        lines.push(Line::styled(
            format!("  Args: {}", truncated),
            Style::default().fg(Color::DarkGray),
        ));

        // Result
        if let Some(ref result) = self.result {
            match result {
                Ok(output) => {
                    let result_str = match output {
                        ToolOutput::Function { content, success } => {
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
        create_diff_summary(&self.summary.changes, &self.summary.cwd, width as usize)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
