//! Bottom pane component with input area and status bar.
//!
//! Provides the user input area and status display at the bottom of the TUI.

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::app_event::AgentStatus;

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
}

impl BottomPane {
    /// Create a new bottom pane.
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
            status: AgentStatus::Idle,
            focused: true,
        }
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

    fn render_status_bar(&self, area: Rect, buf: &mut Buffer) {
        let status_text = match self.status {
            AgentStatus::Idle => "● Ready",
            AgentStatus::Thinking => "● Thinking...",
            AgentStatus::ExecutingTool => "● Executing tool...",
        };

        let status_color = match self.status {
            AgentStatus::Idle => Color::Green,
            AgentStatus::Thinking | AgentStatus::ExecutingTool => Color::Yellow,
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
            (
                Text::styled("Type your message...", Style::default().fg(Color::DarkGray)),
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
