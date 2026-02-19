//! Bottom pane component with input area and status bar.
//!
//! Provides the user input area and status display at the bottom of the TUI.

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::app_event::AgentStatus;

/// Snapshot of user-input question data for rendering (avoids borrowing the request).
#[derive(Clone)]
struct UserInputQuestionSnapshot {
    header: String,
    question: String,
    options: Vec<(String, String)>, // (label, description)
}

/// The bottom pane containing input area and status.
pub struct BottomPane {
    /// Current input text.
    pub input: String,
    /// Cursor position in the input.
    pub cursor_pos: usize,
    /// Current agent status.
    pub status: AgentStatus,
    /// Whether the input is focused.
    pub focused: bool,
    /// Snapshot of the current user-input questions (while awaiting input).
    user_input_questions: Option<Vec<UserInputQuestionSnapshot>>,
    /// Index of the question currently being answered (driven by App).
    pub user_input_current_question: usize,
    /// Currently selected option index (driven by App).
    pub user_input_selected_option: usize,
}

impl BottomPane {
    /// Create a new bottom pane.
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
            status: AgentStatus::Idle,
            focused: true,
            user_input_questions: None,
            user_input_current_question: 0,
            user_input_selected_option: 0,
        }
    }

    /// Store (or clear) the user-input questions to render.
    pub fn set_user_input_questions(
        &mut self,
        questions: Option<&[crate::internal::ai::tools::context::UserInputQuestion]>,
    ) {
        self.user_input_questions = questions.map(|qs| {
            qs.iter()
                .map(|q| UserInputQuestionSnapshot {
                    header: q.header.clone(),
                    question: q.question.clone(),
                    options: q
                        .options
                        .iter()
                        .map(|o| (o.label.clone(), o.description.clone()))
                        .collect(),
                })
                .collect()
        });
        self.user_input_current_question = 0;
        self.user_input_selected_option = 0;
    }

    /// Handle a character input.
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    /// Handle backspace.
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            // Find the start of the previous character
            let prev_pos = self.prev_char_pos();
            self.input.remove(prev_pos);
            self.cursor_pos = prev_pos;
        }
    }

    /// Handle delete.
    pub fn delete(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
        }
    }

    /// Move cursor left.
    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos = self.prev_char_pos();
        }
    }

    /// Move cursor right.
    pub fn cursor_right(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.cursor_pos = self.next_char_pos();
        }
    }

    /// Move cursor to start.
    pub fn cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    /// Move cursor to end.
    pub fn cursor_end(&mut self) {
        self.cursor_pos = self.input.len();
    }

    /// Clear the input.
    pub fn clear(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Get the current input text and clear.
    pub fn take_input(&mut self) -> String {
        let input = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        input
    }

    /// Set the agent status.
    pub fn set_status(&mut self, status: AgentStatus) {
        self.status = status;
    }

    /// Check if input is empty.
    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// Render the bottom pane.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        if self.status == AgentStatus::AwaitingUserInput {
            return self.render_user_input_mode(area, buf);
        }

        // Split area into status bar and input area
        let chunks = Layout::vertical([
            Constraint::Length(1), // Status bar
            Constraint::Length(3), // Input area
            Constraint::Length(1), // Help text
        ])
        .split(area);

        // Render status bar
        self.render_status_bar(chunks[0], buf);

        // Render input area
        let cursor_pos = self.render_input_area(chunks[1], buf);

        // Render help text
        self.render_help_text(chunks[2], buf);

        cursor_pos
    }

    /// Render the bottom pane in user-input mode (questions + options).
    fn render_user_input_mode(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let questions = match &self.user_input_questions {
            Some(q) => q,
            None => return None,
        };

        let q_idx = self.user_input_current_question;
        let question = questions.get(q_idx)?;

        // Count how many lines we need for the question display:
        // 1 status bar + 1 header + 1 question + N options + 1 custom + 1 input + 1 help
        let option_lines = question.options.len() as u16 + 1; // +1 for "Other"
        let question_area_height = 1 + 1 + option_lines; // header + question + options

        let chunks = Layout::vertical([
            Constraint::Length(1),                    // Status bar
            Constraint::Length(question_area_height), // Question + options
            Constraint::Length(3),                    // Input area (for custom text)
            Constraint::Length(1),                    // Help text
        ])
        .split(area);

        // Status bar
        let status_line = Line::styled(
            "● Awaiting input...",
            Style::default().fg(Color::Magenta).bold(),
        );
        Paragraph::new(status_line).render(chunks[0], buf);

        // Question display
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Header
        lines.push(Line::styled(
            format!("  {}", question.header),
            Style::default().fg(Color::Cyan).bold(),
        ));

        // Question text
        lines.push(Line::styled(
            format!("  {}", question.question),
            Style::default().fg(Color::White),
        ));

        // Options
        let selected = self.user_input_selected_option;
        for (i, (label, description)) in question.options.iter().enumerate() {
            let marker = if i == selected { "▸" } else { " " };
            let style = if i == selected {
                Style::default().fg(Color::Cyan).bold()
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::styled(
                format!("  {} {}. {} - {}", marker, i + 1, label, description),
                style,
            ));
        }

        // "Other (custom)" option
        let other_idx = question.options.len();
        let marker = if selected == other_idx { "▸" } else { " " };
        let style = if selected == other_idx {
            Style::default().fg(Color::Cyan).bold()
        } else {
            Style::default().fg(Color::DarkGray)
        };
        lines.push(Line::styled(
            format!("  {} {}. Other (type below)", marker, other_idx + 1),
            style,
        ));

        // Render question area (clip to available height)
        let max_lines = chunks[1].height as usize;
        let display_lines: Vec<Line<'static>> = lines.into_iter().take(max_lines).collect();
        let text = Text::from(display_lines);
        Paragraph::new(text).render(chunks[1], buf);

        // Input area (for custom text entry)
        let cursor_pos = self.render_input_area(chunks[2], buf);

        // Help text
        let help = "[↑/↓: Select] [1-9: Quick select] [Enter: Submit] [Esc: Cancel]";
        let help_line = Line::styled(help, Style::default().fg(Color::DarkGray));
        Paragraph::new(help_line).render(chunks[3], buf);

        // Only show cursor when "Other" is selected
        if selected == other_idx {
            cursor_pos
        } else {
            None
        }
    }

    fn render_status_bar(&self, area: Rect, buf: &mut Buffer) {
        let status_text = match self.status {
            AgentStatus::Idle => "● Ready",
            AgentStatus::Thinking => "● Thinking...",
            AgentStatus::ExecutingTool => "● Executing tool...",
            AgentStatus::AwaitingUserInput => "● Awaiting input...",
        };

        let status_color = match self.status {
            AgentStatus::Idle => Color::Green,
            AgentStatus::Thinking | AgentStatus::ExecutingTool => Color::Yellow,
            AgentStatus::AwaitingUserInput => Color::Magenta,
        };

        let status_line = Line::styled(status_text, Style::default().fg(status_color).bold());
        Paragraph::new(status_line).render(area, buf);
    }

    fn render_input_area(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let border_style = if self.focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);

        let content_width = inner.width as usize;
        let (display_text, cursor_x) = if self.input.is_empty() {
            let placeholder = if self.status == AgentStatus::AwaitingUserInput {
                "Type custom answer..."
            } else {
                "Type your message..."
            };
            (
                Text::styled(placeholder, Style::default().fg(Color::DarkGray)),
                0u16,
            )
        } else {
            let (visible, cursor_x) = self.visible_input_and_cursor_x(content_width);
            (Text::raw(visible), cursor_x)
        };

        Paragraph::new(display_text).block(block).render(area, buf);

        if !self.focused || inner.width == 0 || inner.height == 0 {
            return None;
        }

        Some(Position {
            x: inner.x.saturating_add(cursor_x),
            y: inner.y,
        })
    }

    fn render_help_text(&self, area: Rect, buf: &mut Buffer) {
        let help = match self.status {
            AgentStatus::Idle => {
                "[Enter: Send] [PgUp/PgDn/↑/↓: Scroll] [Ctrl+K: Clear] [Ctrl+C: Exit]"
            }
            AgentStatus::Thinking | AgentStatus::ExecutingTool => {
                "[Esc: Interrupt] [PgUp/PgDn/↑/↓: Scroll] [Ctrl+C: Exit]"
            }
            AgentStatus::AwaitingUserInput => {
                "[↑/↓: Select] [1-9: Quick select] [Enter: Submit] [Esc: Cancel]"
            }
        };

        let help_line = Line::styled(help, Style::default().fg(Color::DarkGray));
        Paragraph::new(help_line).render(area, buf);
    }

    fn prev_char_pos(&self) -> usize {
        let s = &self.input[..self.cursor_pos];
        s.char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
    }

    fn next_char_pos(&self) -> usize {
        let s = &self.input[self.cursor_pos..];
        s.char_indices()
            .nth(1)
            .map(|(i, _)| self.cursor_pos + i)
            .unwrap_or(self.input.len())
    }

    fn visible_input_and_cursor_x(&self, content_width: usize) -> (String, u16) {
        if content_width == 0 {
            return (String::new(), 0);
        }

        let prefix = &self.input[..self.cursor_pos.min(self.input.len())];
        let cursor_col = prefix.width();

        let max_cursor_col = content_width.saturating_sub(1);
        let scroll_cols = cursor_col.saturating_sub(max_cursor_col);

        let start_byte = byte_index_at_display_col(&self.input, scroll_cols);
        let visible = take_by_display_width(&self.input[start_byte..], content_width);

        let cursor_x = (cursor_col.saturating_sub(scroll_cols)).min(u16::MAX as usize) as u16;

        (visible.to_string(), cursor_x)
    }
}

fn byte_index_at_display_col(s: &str, col: usize) -> usize {
    if col == 0 {
        return 0;
    }

    let mut acc = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > col {
            return byte_idx;
        }
        acc += w;
    }
    s.len()
}

fn take_by_display_width(s: &str, max_cols: usize) -> &str {
    if max_cols == 0 {
        return "";
    }
    let mut acc = 0usize;
    let mut end = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > max_cols {
            break;
        }
        acc += w;
        end = byte_idx + ch.len_utf8();
    }
    &s[..end.min(s.len())]
}

impl Default for BottomPane {
    fn default() -> Self {
        Self::new()
    }
}
