//! Main chat widget for displaying conversation history.
//!
//! Renders the scrollable chat area with history cells.

use ratatui::{
    prelude::*,
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use super::{bottom_pane::BottomPane, history_cell::HistoryCell};

/// The main chat widget displaying conversation history.
pub struct ChatWidget {
    /// History cells to display.
    pub cells: Vec<Box<dyn HistoryCell>>,
    /// Number of lines scrolled up from the bottom. `0` means pinned to bottom.
    pub scroll_from_bottom_lines: usize,
    /// Bottom pane for input.
    pub bottom_pane: BottomPane,
    /// Last rendered input area rectangle (for mouse hit-testing).
    last_input_area: Option<Rect>,
    /// Last rendered chat area width used to estimate added line count.
    last_chat_area_width: u16,
}

impl ChatWidget {
    /// Create a new chat widget.
    pub fn new() -> Self {
        Self {
            cells: Vec::new(),
            scroll_from_bottom_lines: 0,
            bottom_pane: BottomPane::new(),
            last_input_area: None,
            last_chat_area_width: 80,
        }
    }

    /// Add a cell to the history.
    pub fn add_cell(&mut self, cell: Box<dyn HistoryCell>) {
        if self.scroll_from_bottom_lines > 0 {
            self.scroll_from_bottom_lines = self
                .scroll_from_bottom_lines
                .saturating_add(cell.desired_height(self.last_chat_area_width) as usize);
        }
        self.cells.push(cell);
    }

    /// Insert a cell at a specific index.
    pub fn insert_cell(&mut self, index: usize, cell: Box<dyn HistoryCell>) {
        if self.scroll_from_bottom_lines > 0 {
            self.scroll_from_bottom_lines = self
                .scroll_from_bottom_lines
                .saturating_add(cell.desired_height(self.last_chat_area_width) as usize);
        }
        self.cells.insert(index, cell);
    }

    /// Scroll up by N lines.
    pub fn scroll_up_lines(&mut self, lines: usize) {
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.saturating_add(lines);
    }

    /// Scroll down by N lines.
    pub fn scroll_down_lines(&mut self, lines: usize) {
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.saturating_sub(lines);
    }

    /// Scroll to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom_lines = 0;
    }

    /// Scroll to the top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_from_bottom_lines = usize::MAX;
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.cells.clear();
        self.scroll_from_bottom_lines = 0;
    }

    pub fn is_in_input_area(&self, x: u16, y: u16) -> bool {
        self.last_input_area.is_some_and(|rect| {
            x >= rect.x
                && x < rect.x.saturating_add(rect.width)
                && y >= rect.y
                && y < rect.y.saturating_add(rect.height)
        })
    }

    /// Render the chat widget.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        // Dynamically size the bottom pane based on current state.
        let bottom_height = self.bottom_pane.desired_height();

        // Split into chat area and bottom pane
        let chunks = Layout::vertical([
            Constraint::Min(3),                    // Chat area (min 3 lines)
            Constraint::Length(bottom_height),      // Bottom pane (dynamic)
        ])
        .split(area);

        // Render chat area
        self.render_chat_area(chunks[0], buf);

        // Track input area for mouse hit-testing (approximate: the input box
        // is at roughly rows 1..4 within the bottom pane).
        let input_y = chunks[1].y.saturating_add(1);
        self.last_input_area = Some(Rect {
            x: chunks[1].x,
            y: input_y,
            width: chunks[1].width,
            height: 3.min(chunks[1].height.saturating_sub(2)),
        });

        self.bottom_pane.render(chunks[1], buf)
    }

    fn render_chat_area(&mut self, area: Rect, buf: &mut Buffer) {
        self.last_chat_area_width = area.width;

        // Calculate visible lines
        let mut lines: Vec<Line<'static>> = Vec::new();

        for cell in &self.cells {
            lines.extend(cell.display_lines(area.width));
        }

        let visible_lines = area.height as usize;
        let total_lines = lines.len();

        let max_scroll_from_bottom = total_lines.saturating_sub(visible_lines);
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.min(max_scroll_from_bottom);

        let start_line = total_lines
            .saturating_sub(visible_lines)
            .saturating_sub(self.scroll_from_bottom_lines);

        let text = Text::from(lines);
        ratatui::widgets::Paragraph::new(text)
            .scroll((start_line.min(u16::MAX as usize) as u16, 0))
            .render(area, buf);

        // Render scrollbar if needed
        if total_lines > visible_lines {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let mut scrollbar_state = ScrollbarState::new(total_lines)
                .position(start_line)
                .viewport_content_length(visible_lines);

            // Note: ratatui 0.29 uses (area, buf, state) order for stateful widgets
            ratatui::widgets::StatefulWidget::render(scrollbar, area, buf, &mut scrollbar_state);
        }
    }
}

impl Default for ChatWidget {
    fn default() -> Self {
        Self::new()
    }
}
