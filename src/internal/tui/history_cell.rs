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
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        lines.push(Line::raw(""));

        let message_lines = self.message.lines();
        let mut is_first_line = true;

        for line in message_lines {
            if line.trim().is_empty() {
                lines.push(Line::raw(""));
            } else if line.starts_with("```") || line.starts_with("    ") || line.starts_with("\t")
            {
                lines.push(Line::styled(
                    format!("  {}", line),
                    Style::default().fg(Color::Yellow),
                ));
            } else {
                let prefix = if is_first_line {
                    "› ".to_string()
                } else {
                    "  ".to_string()
                };
                lines.push(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(Color::Green)),
                    Span::styled(line.to_owned(), Style::default().fg(Color::White)),
                ]));
                is_first_line = false;
            }
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
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        lines.push(Line::raw(""));

        let content = self.content.trim();
        if content.is_empty() {
            if self.is_streaming {
                lines.push(Line::styled(
                    "• Thinking...",
                    Style::default().fg(Color::DarkGray),
                ));
            }
        } else {
            let mut is_first_line = true;
            for line in content.lines() {
                if line.trim().is_empty() {
                    lines.push(Line::raw(""));
                } else if line.starts_with("```")
                    || line.starts_with("    ")
                    || line.starts_with("\t")
                {
                    lines.push(Line::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::Yellow),
                    ));
                } else {
                    let prefix = if is_first_line {
                        "• ".to_string()
                    } else {
                        "  ".to_string()
                    };
                    lines.push(Line::from(vec![
                        Span::styled(prefix, Style::default().fg(Color::Blue)),
                        Span::styled(line.to_owned(), Style::default().fg(Color::White)),
                    ]));
                    is_first_line = false;
                }
            }
        }

        if self.is_streaming {
            lines.push(Line::styled("• ▌", Style::default().fg(Color::Green)));
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
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Status icon and tool name (codex-style)
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

        // Tool header with codex-style
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
                    lines.push(Line::styled(
                        format!("  Error: {}", e),
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
