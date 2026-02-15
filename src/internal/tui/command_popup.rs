use ratatui::{
    buffer::Buffer,
    layout::Rect,
    prelude::Widget,
    text::{Line, Span, Text},
    widgets::Paragraph,
};

use super::slash_command::get_commands_for_input;

const COMMAND_POPUP_MAX_ITEMS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandPopupItem {
    pub command: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct CommandPopup {
    pub commands: Vec<(&'static str, &'static str)>,
    pub selected_index: usize,
    pub visible: bool,
    pub input: String,
}

impl CommandPopup {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            selected_index: 0,
            visible: false,
            input: String::new(),
        }
    }

    pub fn update(&mut self, input: &str) {
        self.input = input.to_string();
        self.commands = get_commands_for_input(input);
        self.visible = !self.commands.is_empty();
        self.selected_index = self
            .selected_index
            .min(self.commands.len().saturating_sub(1));
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.commands.clear();
    }

    pub fn height(&self) -> u16 {
        if self.visible {
            self.commands.len().min(COMMAND_POPUP_MAX_ITEMS) as u16
        } else {
            0
        }
    }

    pub fn move_up(&mut self) {
        if !self.commands.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.commands.len() - 1
            } else {
                self.selected_index - 1
            };
        }
    }

    pub fn move_down(&mut self) {
        if !self.commands.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.commands.len();
        }
    }

    pub fn selected_command(&self) -> Option<(&'static str, &'static str)> {
        self.commands.get(self.selected_index).copied()
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if !self.visible || self.commands.is_empty() {
            return;
        }

        let items = self.get_popup_items(COMMAND_POPUP_MAX_ITEMS);
        let mut lines = Vec::with_capacity(items.len());

        for item in items {
            let prefix = if item.selected { ">" } else { " " };
            let line = Line::from(vec![
                Span::styled(
                    format!("{prefix} {:<18}", item.usage),
                    if item.selected {
                        ratatui::style::Style::default().fg(ratatui::style::Color::LightGreen)
                    } else {
                        ratatui::style::Style::default().fg(ratatui::style::Color::White)
                    },
                ),
                Span::styled(
                    item.description,
                    if item.selected {
                        ratatui::style::Style::default().fg(ratatui::style::Color::LightGreen)
                    } else {
                        ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray)
                    },
                ),
            ]);
            lines.push(line);
        }

        let popup = Paragraph::new(Text::from(lines));
        popup.render(area, buf);
    }

    fn get_popup_items(&self, max_items: usize) -> Vec<CommandPopupItem> {
        if max_items == 0 || self.commands.is_empty() {
            return Vec::new();
        }

        let selected = self.selected_index.min(self.commands.len() - 1);
        let mut start = 0usize;
        if self.commands.len() > max_items && selected >= max_items {
            start = selected + 1 - max_items;
        }
        let end = (start + max_items).min(self.commands.len());

        self.commands[start..end]
            .iter()
            .enumerate()
            .map(|(idx, (name, desc))| {
                let absolute = start + idx;
                CommandPopupItem {
                    command: name,
                    usage: name,
                    description: desc,
                    selected: absolute == selected,
                }
            })
            .collect()
    }
}

impl Default for CommandPopup {
    fn default() -> Self {
        Self::new()
    }
}
