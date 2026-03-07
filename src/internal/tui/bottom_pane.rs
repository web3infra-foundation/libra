//! Bottom pane component with input area and status bar.
//!
//! Provides the user input area and status display at the bottom of the TUI.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::app_event::AgentStatus;

/// Snapshot of user-input question data for rendering (avoids borrowing the request).
#[derive(Clone)]
struct UserInputQuestionSnapshot {
    header: String,
    question: String,
    /// `None` means freeform (text-only).
    options: Option<Vec<(String, String)>>, // (label, description)
    /// Whether a "None of the above" option should be appended.
    is_other: bool,
    /// Whether typed text should be masked (reserved for future use).
    #[allow(dead_code)]
    is_secret: bool,
}

/// State for the slash-command autocomplete popup.
struct CommandPopupState {
    /// Known commands: `(name, description)`, set once at startup.
    commands: Vec<(String, String)>,
    /// Whether the popup is currently visible.
    visible: bool,
    /// Index of the currently highlighted command in the *filtered* list.
    selected: usize,
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
    /// Whether the notes input is currently focused (driven by App).
    pub user_input_notes_focused: bool,
    /// Current notes text (driven by App).
    pub user_input_notes_text: String,
    /// Slash-command autocomplete popup state.
    command_popup: CommandPopupState,
    /// Currently selected option in the post-plan dialog (0=Execute, 1=Modify, 2=Cancel).
    pub post_plan_selected: usize,
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
            user_input_notes_focused: false,
            user_input_notes_text: String::new(),
            command_popup: CommandPopupState {
                commands: Vec::new(),
                visible: false,
                selected: 0,
            },
            post_plan_selected: 0,
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
                    options: q.options.as_ref().map(|opts| {
                        opts.iter()
                            .map(|o| (o.label.clone(), o.description.clone()))
                            .collect()
                    }),
                    is_other: q.is_other,
                    is_secret: q.is_secret,
                })
                .collect()
        });
        self.user_input_current_question = 0;
        self.user_input_selected_option = 0;
        self.user_input_notes_focused = false;
        self.user_input_notes_text.clear();
    }

    /// Reset the post-plan dialog selection.
    pub fn reset_post_plan_selection(&mut self) {
        self.post_plan_selected = 0;
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

    // ── Slash-command autocomplete popup ────────────────────────────

    /// Set the known slash commands (called once at startup).
    pub fn set_command_hints(&mut self, commands: Vec<(String, String)>) {
        self.command_popup.commands = commands;
    }

    /// Whether the command popup is currently visible.
    pub fn is_command_popup_visible(&self) -> bool {
        self.command_popup.visible
    }

    /// Synchronise popup visibility after every input mutation.
    ///
    /// Shows the popup when the input starts with `/` and contains no space;
    /// hides it otherwise. Clamps the selection index to the filtered list.
    pub fn sync_command_popup(&mut self) {
        let should_show = self.input.starts_with('/')
            && !self.input.contains(' ')
            && !self.command_popup.commands.is_empty();

        self.command_popup.visible = should_show;

        if should_show {
            let count = self.filtered_commands().len();
            if count == 0 {
                self.command_popup.visible = false;
            } else if self.command_popup.selected >= count {
                self.command_popup.selected = count.saturating_sub(1);
            }
        } else {
            self.command_popup.selected = 0;
        }
    }

    /// Hide the popup (Esc).
    pub fn dismiss_command_popup(&mut self) {
        self.command_popup.visible = false;
        self.command_popup.selected = 0;
    }

    /// Move selection up in the popup.
    pub fn command_popup_up(&mut self) {
        if self.command_popup.selected > 0 {
            self.command_popup.selected -= 1;
        }
    }

    /// Move selection down in the popup.
    pub fn command_popup_down(&mut self) {
        let count = self.filtered_commands().len();
        if count > 0 && self.command_popup.selected < count - 1 {
            self.command_popup.selected += 1;
        }
    }

    /// Complete the selected command (Tab).
    ///
    /// Replaces the input with `/<name> ` and moves cursor to end.
    /// Returns `true` if a completion was performed.
    pub fn complete_command(&mut self) -> bool {
        let filtered = self.filtered_commands();
        if let Some((name, _)) = filtered.get(self.command_popup.selected) {
            let completed = format!("/{} ", name);
            self.input = completed;
            self.cursor_pos = self.input.len();
            self.command_popup.visible = false;
            self.command_popup.selected = 0;
            true
        } else {
            false
        }
    }

    /// Return the commands matching the current prefix (after `/`), case-insensitive.
    fn filtered_commands(&self) -> Vec<(String, String)> {
        let prefix = self.input.strip_prefix('/').unwrap_or("");
        let prefix_lower = prefix.to_lowercase();
        self.command_popup
            .commands
            .iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&prefix_lower))
            .cloned()
            .collect()
    }

    /// Check if input is empty.
    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// Return the height (in lines) the bottom pane needs for the current state.
    pub fn desired_height(&self) -> u16 {
        if self.status == AgentStatus::AwaitingPostPlanChoice {
            // status(1) + 3 options + 1 blank + help(1) = 6
            return 6;
        }
        if self.status != AgentStatus::AwaitingUserInput {
            // Normal mode: rounded input box(5, with 3-line input inner area) + statusline(1) = 6
            return 6;
        }

        let questions = match &self.user_input_questions {
            Some(q) => q,
            None => return 5,
        };

        let q_idx = self.user_input_current_question;
        let question = match questions.get(q_idx) {
            Some(q) => q,
            None => return 5,
        };

        let options = question.options.as_deref().unwrap_or_default();
        let is_freeform = options.is_empty();

        let option_lines = if is_freeform {
            0u16
        } else {
            let extra = if question.is_other { 1 } else { 0 };
            options.len() as u16 + extra
        };

        let question_area = 1 + 1 + option_lines; // header + question + options
        let notes_height = if !is_freeform { 3u16 } else { 0 };

        // status(1) + question_area + input(3) + notes + help(1)
        1 + question_area + 3 + notes_height + 1
    }

    /// Render the bottom pane.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        if self.status == AgentStatus::AwaitingUserInput {
            return self.render_user_input_mode(area, buf);
        }
        if self.status == AgentStatus::AwaitingPostPlanChoice {
            return self.render_post_plan_dialog(area, buf);
        }

        // Split area into input area and status bar.
        let chunks = Layout::vertical([
            Constraint::Length(5), // Rounded input box (3-line inner input)
            Constraint::Length(1), // Status line
        ])
        .split(area);

        // Render input area
        let cursor_pos = self.render_input_area(chunks[0], buf);
        // Render status bar below the input box
        self.render_status_bar(chunks[1], buf);

        // Render command popup (floats above the bottom pane)
        if self.command_popup.visible && self.status == AgentStatus::Idle {
            self.render_command_popup(area, buf);
        }

        cursor_pos
    }

    /// Return the clickable input hitbox for focus handling.
    pub fn input_hitbox(&self, area: Rect) -> Option<Rect> {
        if self.status == AgentStatus::AwaitingPostPlanChoice {
            return None;
        }

        if self.status != AgentStatus::AwaitingUserInput {
            let chunks =
                Layout::vertical([Constraint::Length(5), Constraint::Length(1)]).split(area);
            return Some(chunks[0]);
        }

        let questions = self.user_input_questions.as_ref()?;
        let question = questions.get(self.user_input_current_question)?;
        let options = question.options.as_deref().unwrap_or_default();
        let is_freeform = options.is_empty();

        let option_lines = if is_freeform {
            0u16
        } else {
            let extra = if question.is_other { 1u16 } else { 0 };
            options.len() as u16 + extra
        };
        let question_area_height = 1 + 1 + option_lines;
        let notes_height = if !is_freeform { 3u16 } else { 0 };

        let chunks = Layout::vertical([
            Constraint::Length(1),                    // Status bar
            Constraint::Length(question_area_height), // Question + options
            Constraint::Length(3),                    // Freeform input
            Constraint::Length(notes_height),         // Notes
            Constraint::Length(1),                    // Help text
        ])
        .split(area);

        let hit = if is_freeform { chunks[2] } else { chunks[3] };
        if hit.width > 0 && hit.height > 0 {
            Some(hit)
        } else {
            None
        }
    }

    /// Render the bottom pane in user-input mode (questions + options).
    fn render_user_input_mode(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let questions = match &self.user_input_questions {
            Some(q) => q,
            None => return None,
        };

        let q_idx = self.user_input_current_question;
        let question = questions.get(q_idx)?;
        let total_questions = questions.len();

        let options = question.options.as_deref().unwrap_or_default();
        let is_freeform = options.is_empty();

        // Count how many lines we need for the question display
        let option_lines = if is_freeform {
            0u16
        } else {
            let extra = if question.is_other { 1u16 } else { 0 };
            options.len() as u16 + extra
        };
        let question_area_height = 1 + 1 + option_lines; // progress + question + options
        let notes_height = if !is_freeform { 3u16 } else { 0 }; // notes area (only for option questions)

        let chunks = Layout::vertical([
            Constraint::Length(1),                    // Status bar
            Constraint::Length(question_area_height), // Question + options
            Constraint::Length(3),                    // Input area (freeform or notes)
            Constraint::Length(notes_height),         // Notes area (for option questions)
            Constraint::Length(1),                    // Help text
        ])
        .split(area);

        // Status bar
        let progress = if total_questions > 1 {
            format!(
                "● Question {}/{} — Awaiting input...",
                q_idx + 1,
                total_questions
            )
        } else {
            "● Awaiting input...".to_string()
        };
        let status_line = Line::styled(progress, Style::default().fg(Color::Magenta).bold());
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
            Style::default(),
        ));

        let selected = self.user_input_selected_option;

        if !is_freeform {
            // Render predefined options
            for (i, (label, description)) in options.iter().enumerate() {
                let marker = if i == selected && !self.user_input_notes_focused {
                    "›"
                } else {
                    " "
                };
                let style = if i == selected {
                    Style::default().fg(Color::Cyan).bold()
                } else {
                    Style::default()
                };
                lines.push(Line::styled(
                    format!("  {} {}. {}  {}", marker, i + 1, label, description),
                    style,
                ));
            }

            // "None of the above" option (when is_other is set)
            if question.is_other {
                let other_idx = options.len();
                let marker = if selected == other_idx && !self.user_input_notes_focused {
                    "›"
                } else {
                    " "
                };
                let style = if selected == other_idx {
                    Style::default().fg(Color::Cyan).bold()
                } else {
                    Style::default().add_modifier(Modifier::DIM)
                };
                lines.push(Line::styled(
                    format!(
                        "  {} {}. None of the above  Optionally, add details in notes (tab).",
                        marker,
                        other_idx + 1
                    ),
                    style,
                ));
            }
        }

        // Render question area (clip to available height)
        let max_lines = chunks[1].height as usize;
        let display_lines: Vec<Line<'static>> = lines.into_iter().take(max_lines).collect();
        let text = Text::from(display_lines);
        Paragraph::new(text).render(chunks[1], buf);

        // Input area — for freeform questions, this is the primary input.
        // For option questions, this area is used for notes when Tab is pressed.
        let cursor_pos = if is_freeform {
            self.render_input_area(chunks[2], buf)
        } else {
            // Show notes input area
            self.render_notes_area(chunks[3], buf)
        };

        // Help text
        let help = if is_freeform {
            "[Enter: Submit] [Esc: Cancel]"
        } else {
            "[↑/↓: Select] [1-9: Quick select] [Tab: Notes] [Enter: Submit] [Esc: Cancel]"
        };
        let help_line = Line::styled(help, Style::default().add_modifier(Modifier::DIM));
        Paragraph::new(help_line).render(chunks[4], buf);

        // Show cursor: for freeform always, for options only when notes focused
        if is_freeform || self.user_input_notes_focused {
            cursor_pos
        } else {
            None
        }
    }

    /// Render the post-plan dialog (Execute / Modify / Cancel).
    fn render_post_plan_dialog(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let chunks = Layout::vertical([
            Constraint::Length(1), // Status bar
            Constraint::Length(4), // 3 options + 1 blank line
            Constraint::Length(1), // Help text
        ])
        .split(area);

        // Status bar
        self.render_status_bar(chunks[0], buf);

        // Options
        let options = [
            ("Execute Spec", "Run the orchestrator"),
            ("Modify Spec", "Edit the plan"),
            ("Cancel", "Return to chat"),
        ];

        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, (label, desc)) in options.iter().enumerate() {
            let marker = if i == self.post_plan_selected {
                "▸"
            } else {
                " "
            };
            let style = if i == self.post_plan_selected {
                Style::default().fg(Color::Cyan).bold()
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::styled(
                format!("  {} {:<16} {}", marker, label, desc),
                style,
            ));
        }
        lines.push(Line::raw(""));

        Paragraph::new(Text::from(lines)).render(chunks[1], buf);

        // Help text
        self.render_help_text(chunks[2], buf);

        None // no cursor in this mode
    }

    /// Render the notes input area for option questions.
    fn render_notes_area(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        if area.height == 0 {
            return None;
        }

        let border_style = if self.user_input_notes_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Notes ");

        let inner = block.inner(area);

        let display = if self.user_input_notes_text.is_empty() {
            Text::styled(
                "Tab to add notes...",
                Style::default().add_modifier(Modifier::DIM),
            )
        } else {
            Text::raw(&self.user_input_notes_text)
        };

        Paragraph::new(display).block(block).render(area, buf);

        if self.user_input_notes_focused && inner.width > 0 && inner.height > 0 {
            let cursor_x = self.user_input_notes_text.width().min(inner.width as usize) as u16;
            Some(Position {
                x: inner.x.saturating_add(cursor_x),
                y: inner.y,
            })
        } else {
            None
        }
    }

    /// Render the slash-command autocomplete popup floating above the bottom pane.
    fn render_command_popup(&self, bottom_area: Rect, buf: &mut Buffer) {
        let filtered = self.filtered_commands();
        if filtered.is_empty() {
            return;
        }

        let max_visible: u16 = 8;
        let item_count = filtered.len() as u16;
        // +2 for border top/bottom
        let popup_height = item_count.min(max_visible) + 2;

        // Position the popup just above the bottom pane area.
        let popup_y = bottom_area.y.saturating_sub(popup_height);
        let popup_area = Rect {
            x: bottom_area.x,
            y: popup_y,
            width: bottom_area.width,
            height: popup_height,
        };

        // Ensure the popup area is within the buffer bounds.
        let buf_area = *buf.area();
        let clamped = popup_area.intersection(buf_area);
        if clamped.width == 0 || clamped.height == 0 {
            return;
        }

        // Clear the region behind the popup.
        Clear.render(clamped, buf);

        // Build the list lines.
        let inner_width = clamped.width.saturating_sub(2) as usize; // borders
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, (name, desc)) in filtered.iter().enumerate() {
            let is_selected = i == self.command_popup.selected;
            let prefix = format!("  /{:<16}", name);
            let remaining = inner_width.saturating_sub(prefix.len());
            let truncated_desc: String = if desc.len() > remaining {
                let max_chars = remaining.saturating_sub(2);
                let truncated: String = desc.chars().take(max_chars).collect();
                format!("{truncated}..")
            } else {
                desc.clone()
            };
            let text = format!("{}{}", prefix, truncated_desc);
            let style = if is_selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::styled(text, style));
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().add_modifier(Modifier::DIM))
            .title(" Commands ");

        let paragraph = Paragraph::new(Text::from(lines)).block(block);
        paragraph.render(clamped, buf);
    }

    fn render_status_bar(&self, area: Rect, buf: &mut Buffer) {
        let phase = animation_phase(120);
        let status_line = match self.status {
            AgentStatus::Idle => Line::styled("● Ready", Style::default().fg(Color::Green).bold()),
            AgentStatus::Thinking => {
                gradient_line("● Thinking...", &thinking_colors(), phase, true)
            }
            AgentStatus::ExecutingTool => {
                gradient_line("● Executing tool...", &executing_tool_colors(), phase, true)
            }
            AgentStatus::AwaitingUserInput => Line::styled(
                "● Awaiting input...",
                Style::default().fg(Color::Magenta).bold(),
            ),
            AgentStatus::AwaitingPostPlanChoice => Line::styled(
                "● Plan complete — choose next step",
                Style::default().fg(Color::Magenta).bold(),
            ),
        };
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
            .border_type(BorderType::Rounded)
            .border_style(border_style);

        let inner = block.inner(area);

        let content_width = inner.width as usize;
        let content_height = inner.height as usize;
        let (display_text, cursor_x, cursor_y) = if self.input.is_empty() {
            let placeholder = if self.status == AgentStatus::AwaitingUserInput {
                "Type custom answer..."
            } else {
                "Type your message..."
            };
            (
                Text::from(vec![Line::styled(
                    placeholder,
                    Style::default().add_modifier(Modifier::DIM),
                )]),
                0u16,
                0u16,
            )
        } else {
            let (lines, cursor_x, cursor_y) =
                self.visible_input_and_cursor(content_width, content_height);
            let text_lines = lines.into_iter().map(Line::raw).collect::<Vec<_>>();
            (Text::from(text_lines), cursor_x, cursor_y)
        };

        Paragraph::new(display_text).block(block).render(area, buf);

        if !self.focused || inner.width == 0 || inner.height == 0 {
            return None;
        }

        Some(Position {
            x: inner.x.saturating_add(cursor_x),
            y: inner.y.saturating_add(cursor_y),
        })
    }

    fn render_help_text(&self, area: Rect, buf: &mut Buffer) {
        let help = match self.status {
            AgentStatus::Idle => {
                if self.command_popup.visible {
                    "[Tab: Complete] [Up/Down: Select] [Esc: Dismiss] [Enter: Send]"
                } else {
                    "[Enter: Send] [PgUp/PgDn: Scroll] [Shift+Drag: Select] [Ctrl+C: Exit]"
                }
            }
            AgentStatus::Thinking | AgentStatus::ExecutingTool => {
                "[Esc: Interrupt] [PgUp/PgDn: Scroll] [Shift+Drag: Select] [Ctrl+C: Exit]"
            }
            AgentStatus::AwaitingUserInput => {
                "[Up/Down: Select] [1-9: Quick select] [Enter: Submit] [Esc: Cancel]"
            }
            AgentStatus::AwaitingPostPlanChoice => {
                "[Up/Down: Select] [Enter: Confirm] [Esc: Cancel]"
            }
        };

        let help_line = Line::styled(help, Style::default().add_modifier(Modifier::DIM));
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

    fn visible_input_and_cursor(
        &self,
        content_width: usize,
        content_height: usize,
    ) -> (Vec<String>, u16, u16) {
        if content_width == 0 || content_height == 0 {
            return (Vec::new(), 0, 0);
        }

        let mut wrapped_lines: Vec<String> = Vec::new();
        let mut current_line = String::new();
        let mut current_col = 0usize;
        let mut cursor_row = 0usize;
        let mut cursor_col = 0usize;
        let mut line_index = 0usize;

        for (idx, ch) in self.input.char_indices() {
            if ch == '\n' {
                if idx == self.cursor_pos {
                    cursor_row = line_index;
                    cursor_col = current_col;
                }
                wrapped_lines.push(std::mem::take(&mut current_line));
                line_index += 1;
                current_col = 0;
                continue;
            }

            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
            if current_col + ch_width > content_width && current_col > 0 {
                wrapped_lines.push(std::mem::take(&mut current_line));
                line_index += 1;
                current_col = 0;
            }

            if idx == self.cursor_pos {
                cursor_row = line_index;
                cursor_col = current_col;
            }

            current_line.push(ch);
            current_col += ch_width;
        }

        if self.cursor_pos == self.input.len() {
            cursor_row = line_index;
            cursor_col = current_col;
        }

        wrapped_lines.push(current_line);

        let start_row = cursor_row.saturating_sub(content_height.saturating_sub(1));
        let end_row = (start_row + content_height).min(wrapped_lines.len());
        let mut visible_lines = wrapped_lines[start_row..end_row].to_vec();
        while visible_lines.len() < content_height {
            visible_lines.push(String::new());
        }

        let cursor_y = cursor_row.saturating_sub(start_row).min(u16::MAX as usize) as u16;
        let max_cursor_x = content_width.saturating_sub(1);
        let cursor_x = cursor_col.min(max_cursor_x).min(u16::MAX as usize) as u16;
        (visible_lines, cursor_x, cursor_y)
    }
}

fn animation_phase(step_ms: u128) -> usize {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    (millis / step_ms.max(1)) as usize
}

fn thinking_colors() -> [Color; 5] {
    [
        Color::Rgb(110, 170, 255),
        Color::Rgb(135, 200, 255),
        Color::Rgb(230, 242, 255),
        Color::Rgb(135, 200, 255),
        Color::Rgb(110, 170, 255),
    ]
}

fn executing_tool_colors() -> [Color; 5] {
    [
        Color::Rgb(120, 190, 255),
        Color::Rgb(120, 230, 210),
        Color::Rgb(230, 250, 240),
        Color::Rgb(120, 230, 210),
        Color::Rgb(120, 190, 255),
    ]
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

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect};

    use super::BottomPane;
    use crate::internal::tui::app_event::AgentStatus;

    fn row_text(buf: &Buffer, y: u16, width: u16) -> String {
        let mut out = String::new();
        for x in 0..width {
            out.push_str(buf[(x, y)].symbol());
        }
        out
    }

    #[test]
    fn normal_mode_height_is_six_lines() {
        let pane = BottomPane::new();
        assert_eq!(pane.desired_height(), 6);
    }

    #[test]
    fn statusline_renders_below_rounded_input_box() {
        let mut pane = BottomPane::new();
        pane.status = AgentStatus::Idle;

        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        let _ = pane.render(area, &mut buf);

        let top = row_text(&buf, 0, area.width);
        let bottom_of_box = row_text(&buf, 4, area.width);
        let status = row_text(&buf, 5, area.width);

        assert!(top.contains("╭"));
        assert!(bottom_of_box.contains("╰"));
        assert!(status.contains("Ready"));
        assert!(!status.contains("Enter: Send"));
    }
}

impl Default for BottomPane {
    fn default() -> Self {
        Self::new()
    }
}
