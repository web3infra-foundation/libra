//! Bottom pane component with input area and status bar.
//!
//! Provides the user input area and status display at the bottom of the TUI.

use ratatui::{prelude::*, widgets::Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{app_event::AgentStatus, command_popup::CommandPopup};

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
    /// Command popup for slash command autocomplete.
    pub command_popup: CommandPopup,
}

impl BottomPane {
    /// Create a new bottom pane.
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
            status: AgentStatus::Idle,
            focused: true,
            command_popup: CommandPopup::new(),
        }
    }

    /// Update command popup based on current input.
    pub fn update_command_popup(&mut self) {
        self.command_popup.update(&self.input);
    }

    /// Move selection up in command popup.
    pub fn popup_move_up(&mut self) {
        self.command_popup.move_up();
    }

    /// Move selection down in command popup.
    pub fn popup_move_down(&mut self) {
        self.command_popup.move_down();
    }

    /// Apply selected command to input.
    pub fn apply_selected_command(&mut self) -> bool {
        if let Some((name, _)) = self.command_popup.selected_command() {
            self.input = format!("/{}", name);
            self.cursor_pos = self.input.len();
            self.command_popup.hide();
            return true;
        }
        false
    }

    /// Hide command popup.
    pub fn hide_popup(&mut self) {
        self.command_popup.hide();
    }

    /// Check if popup is visible.
    pub fn is_popup_visible(&self) -> bool {
        self.command_popup.is_visible()
    }

    /// Handle a character input.
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
        self.update_command_popup();
    }

    /// Handle backspace.
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let prev_pos = self.prev_char_pos();
            self.input.remove(prev_pos);
            self.cursor_pos = prev_pos;
            self.update_command_popup();
        }
    }

    /// Handle delete.
    pub fn delete(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
            self.update_command_popup();
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
        self.hide_popup();
    }

    /// Get the current input text and clear.
    pub fn take_input(&mut self) -> String {
        let input = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        self.hide_popup();
        input
    }

    /// Set the agent status.
    pub fn set_status(&mut self, status: AgentStatus) {
        self.status = status;
    }

    /// Check if the current input is a slash command (starts with /).
    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// Render the bottom pane.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let input_height = 3; // Fixed input height
        let status_height = 1;
        let popup_height = self.command_popup.height();
        let bottom_height = status_height + input_height + popup_height + 1;

        let chunks =
            Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_height)]).split(area);

        let bottom = chunks[1];
        let mut constraints = Vec::new();
        constraints.push(Constraint::Length(status_height));
        constraints.push(Constraint::Length(input_height));
        if popup_height > 0 {
            constraints.push(Constraint::Length(popup_height));
        }
        constraints.push(Constraint::Length(1)); // Help text
        let bottom_chunks = Layout::vertical(constraints).split(bottom);

        let mut idx = 0usize;
        // Render status bar
        self.render_status_bar(bottom_chunks[idx], buf);
        idx += 1;

        // Render input area
        let cursor_pos = self.render_input_area(bottom_chunks[idx], buf);
        idx += 1;

        // Render command popup
        if popup_height > 0 {
            self.command_popup.render(bottom_chunks[idx], buf);
            idx += 1;
        }

        // Render help text
        self.render_help_text(bottom_chunks[idx], buf);

        cursor_pos
    }

    fn render_status_bar(&self, area: Rect, buf: &mut Buffer) {
        let (status_text, status_color) = match self.status {
            AgentStatus::Idle => ("Ready", Color::DarkGray),
            AgentStatus::Thinking => ("Thinking...", Color::DarkGray),
            AgentStatus::ExecutingTool => ("Executing...", Color::DarkGray),
        };

        let status_line = Line::styled(status_text, Style::default().fg(status_color));
        Paragraph::new(status_line).render(area, buf);
    }

    fn render_input_area(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        // Codex style: no border, just a simple input area with a prompt indicator
        let prompt = "›";
        let prompt_style = if self.focused {
            Style::default().fg(Color::White).bold()
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Render prompt indicator at the start
        let span = Span::styled(prompt, prompt_style);
        buf.set_span(area.x, area.y, &span, 1);

        // Input area starts after the prompt
        let input_area = Rect::new(
            area.x + 1,
            area.y,
            area.width.saturating_sub(1),
            area.height,
        );

        let content_width = input_area.width as usize;
        let (display_text, cursor_x) = if self.input.is_empty() {
            (
                Text::styled("Type your message...", Style::default().fg(Color::DarkGray)),
                0u16,
            )
        } else {
            let (visible, cursor_x) = self.visible_input_and_cursor_x(content_width);
            (Text::raw(visible), cursor_x)
        };

        Paragraph::new(display_text).render(input_area, buf);

        if !self.focused || input_area.width == 0 {
            return None;
        }

        Some(Position {
            x: input_area.x.saturating_add(cursor_x),
            y: input_area.y,
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
