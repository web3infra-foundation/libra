//! Main chat widget for displaying conversation history.
//!
//! Renders the scrollable chat area with history cells.

use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use uuid::Uuid;

use super::{bottom_pane::BottomPane, history_cell::HistoryCell, theme};
use crate::internal::ai::orchestrator::types::{ExecutionPlanSpec, TaskKind, TaskNodeStatus};

#[derive(Debug, Clone)]
struct DagPanelNode {
    task_id: Uuid,
    kind: TaskKind,
    dependencies: Vec<Uuid>,
    depth: usize,
    lane: usize,
    ordinal: usize,
    status: TaskNodeStatus,
}

#[derive(Debug, Clone, Default)]
struct DagPanelCell {
    mask: u8,
    animated: bool,
}

#[derive(Debug, Clone)]
struct DagPanelState {
    revision: u32,
    completed: usize,
    total: usize,
    nodes: Vec<DagPanelNode>,
}

impl DagPanelState {
    fn new(plan: ExecutionPlanSpec) -> Self {
        let groups = plan.parallel_groups();
        let mut id_to_depth = std::collections::HashMap::new();
        let mut id_to_lane = std::collections::HashMap::new();
        for (depth, group) in groups.iter().enumerate() {
            for (lane, id) in group.iter().enumerate() {
                id_to_depth.insert(*id, depth);
                id_to_lane.insert(*id, lane);
            }
        }

        let nodes = plan
            .tasks
            .iter()
            .enumerate()
            .map(|(idx, task)| DagPanelNode {
                task_id: task.id(),
                kind: task.kind.clone(),
                dependencies: task.dependencies().to_vec(),
                depth: id_to_depth.get(&task.id()).copied().unwrap_or_default(),
                lane: id_to_lane.get(&task.id()).copied().unwrap_or_default(),
                ordinal: idx + 1,
                status: TaskNodeStatus::Pending,
            })
            .collect::<Vec<_>>();

        Self {
            revision: plan.revision,
            total: plan.tasks.len(),
            completed: 0,
            nodes,
        }
    }

    fn update_task_status(&mut self, task_id: Uuid, status: TaskNodeStatus) {
        if let Some(node) = self.nodes.iter_mut().find(|node| node.task_id == task_id) {
            node.status = status;
        }
    }

    fn update_progress(&mut self, completed: usize, total: usize) {
        self.completed = completed.min(total);
        self.total = total.max(self.total);
    }

    fn lane_count(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.lane)
            .max()
            .map(|max| max + 1)
            .unwrap_or(0)
    }

    fn depth_count(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.depth)
            .max()
            .map(|max| max + 1)
            .unwrap_or(0)
    }

    fn has_running(&self) -> bool {
        self.nodes
            .iter()
            .any(|node| node.status == TaskNodeStatus::Running)
    }

    fn failed_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.status == TaskNodeStatus::Failed)
            .count()
    }

    fn running_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.status == TaskNodeStatus::Running)
            .count()
    }
}

fn animation_phase(step_ms: u128) -> usize {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    (millis / step_ms.max(1)) as usize
}

/// The main chat widget displaying conversation history.
pub struct ChatWidget {
    /// History cells to display.
    pub cells: Vec<Box<dyn HistoryCell>>,
    /// Active DAG panel shown alongside history while a workflow is running.
    dag_panel: Option<DagPanelState>,
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
            dag_panel: None,
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
        self.dag_panel = None;
        self.scroll_from_bottom_lines = 0;
    }

    pub fn show_dag_panel(&mut self, plan: ExecutionPlanSpec) {
        self.dag_panel = Some(DagPanelState::new(plan));
    }

    pub fn update_dag_task_status(&mut self, task_id: Uuid, status: TaskNodeStatus) {
        if let Some(panel) = self.dag_panel.as_mut() {
            panel.update_task_status(task_id, status);
        }
    }

    pub fn update_dag_progress(&mut self, completed: usize, total: usize) {
        if let Some(panel) = self.dag_panel.as_mut() {
            panel.update_progress(completed, total);
        }
    }

    pub fn clear_dag_panel(&mut self) {
        self.dag_panel = None;
    }

    pub fn is_in_input_area(&self, x: u16, y: u16) -> bool {
        self.last_input_area.is_some_and(|rect| {
            x >= rect.x
                && x < rect.x.saturating_add(rect.width)
                && y >= rect.y
                && y < rect.y.saturating_add(rect.height)
        })
    }

    fn split_areas(&self, area: Rect) -> (Rect, Rect) {
        let bottom_height = self.bottom_pane.desired_height();
        let chunks = Layout::vertical([
            Constraint::Min(3),                // Chat area (min 3 lines)
            Constraint::Length(bottom_height), // Bottom pane (dynamic)
        ])
        .split(area);
        (chunks[0], chunks[1])
    }

    /// Render the chat widget.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let (chat_area, bottom_area) = self.split_areas(area);

        // Render chat area
        self.render_chat_area(chat_area, buf);

        // Track the active input area for mouse hit-testing.
        self.last_input_area = self.bottom_pane.input_hitbox(bottom_area);

        self.bottom_pane.render(bottom_area, buf)
    }

    pub fn chat_area_rect(&self, area: Rect) -> Rect {
        self.split_areas(area).0
    }

    pub fn render_bottom_pane_only(&mut self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        let (_, bottom_area) = self.split_areas(area);
        self.last_input_area = self.bottom_pane.input_hitbox(bottom_area);
        self.bottom_pane.render(bottom_area, buf)
    }

    fn render_chat_area(&mut self, area: Rect, buf: &mut Buffer) {
        let (history_area, dag_area) = self.split_chat_columns(area);
        self.last_chat_area_width = history_area.width;

        // Calculate visible lines
        let mut lines: Vec<Line<'static>> = Vec::new();

        for cell in &self.cells {
            lines.extend(cell.display_lines(history_area.width));
        }

        let visible_lines = history_area.height as usize;
        let total_lines = lines.len();

        let max_scroll_from_bottom = total_lines.saturating_sub(visible_lines);
        self.scroll_from_bottom_lines = self.scroll_from_bottom_lines.min(max_scroll_from_bottom);

        let start_line = total_lines
            .saturating_sub(visible_lines)
            .saturating_sub(self.scroll_from_bottom_lines);

        let text = Text::from(lines);
        ratatui::widgets::Paragraph::new(text)
            .scroll((start_line.min(u16::MAX as usize) as u16, 0))
            .render(history_area, buf);

        // Render scrollbar if needed
        if total_lines > visible_lines {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let mut scrollbar_state = ScrollbarState::new(total_lines)
                .position(start_line)
                .viewport_content_length(visible_lines);

            // Note: ratatui 0.29 uses (area, buf, state) order for stateful widgets
            ratatui::widgets::StatefulWidget::render(
                scrollbar,
                history_area,
                buf,
                &mut scrollbar_state,
            );
        }

        if let Some(dag_area) = dag_area {
            self.render_dag_panel(dag_area, buf);
        }
    }

    fn split_chat_columns(&self, area: Rect) -> (Rect, Option<Rect>) {
        let Some(_) = self.dag_panel else {
            return (area, None);
        };
        if area.width < 96 {
            return (area, None);
        }

        let panel_width = (area.width / 3).clamp(30, 42);
        let chunks = Layout::horizontal([
            Constraint::Min(area.width.saturating_sub(panel_width)),
            Constraint::Length(panel_width),
        ])
        .split(area);
        (chunks[0], Some(chunks[1]))
    }

    fn render_dag_panel(&self, area: Rect, buf: &mut Buffer) {
        let Some(panel) = self.dag_panel.as_ref() else {
            return;
        };
        if area.width < 12 || area.height < 8 {
            return;
        }

        let title_style = if panel.has_running() {
            theme::interactive::in_progress()
        } else {
            theme::interactive::title()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::border::idle())
            .title(Line::from(vec![
                Span::styled(
                    format!(" Workflow r{} ", panel.revision),
                    title_style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {} / {} ", panel.completed, panel.total),
                    theme::text::muted(),
                ),
            ]))
            .title_alignment(Alignment::Center);
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 12 || inner.height < 7 {
            return;
        }

        let lane_count = panel.lane_count().max(1);
        let depth_count = panel.depth_count().max(1);
        let graph_top = inner.y.saturating_add(1);
        let graph_height = inner.height.saturating_sub(4) as usize;
        if graph_height == 0 {
            return;
        }
        let row_step = if depth_count <= 1 {
            1usize
        } else {
            graph_height.saturating_sub(1) / depth_count.saturating_sub(1).max(1)
        }
        .max(1);
        let usable_width = inner.width.saturating_sub(2) as usize;
        let lane_span = usable_width.saturating_sub(1);

        let mut node_positions = BTreeMap::new();
        for node in &panel.nodes {
            let x = inner.x as usize
                + 1
                + if lane_count <= 1 {
                    lane_span / 2
                } else {
                    node.lane * lane_span / lane_count.saturating_sub(1)
                };
            let y = graph_top as usize + node.depth * row_step;
            node_positions.insert(node.task_id, (x, y));
        }

        let phase = animation_phase(110);
        let mut edge_cells: BTreeMap<(usize, usize), DagPanelCell> = BTreeMap::new();

        for node in &panel.nodes {
            let Some(&(target_x, target_y)) = node_positions.get(&node.task_id) else {
                continue;
            };
            let connector_y = target_y.saturating_sub(1);
            for dependency_id in &node.dependencies {
                let Some(&(source_x, source_y)) = node_positions.get(dependency_id) else {
                    continue;
                };
                let animated = node.status == TaskNodeStatus::Running
                    || panel
                        .nodes
                        .iter()
                        .find(|candidate| candidate.task_id == *dependency_id)
                        .is_some_and(|candidate| candidate.status == TaskNodeStatus::Running);
                draw_vertical_edge(
                    &mut edge_cells,
                    source_x,
                    source_y.saturating_add(1),
                    connector_y,
                    animated,
                );
                draw_horizontal_edge(&mut edge_cells, connector_y, source_x, target_x, animated);
            }
        }

        for ((x, y), cell) in edge_cells {
            if x < area.x as usize || y < area.y as usize {
                continue;
            }
            let color = if cell.animated {
                theme::animation::executing_gradient()
                    [(x + y + phase) % theme::animation::executing_gradient().len()]
            } else {
                theme::text::subtle().fg.unwrap_or(Color::Reset)
            };
            if x <= u16::MAX as usize && y <= u16::MAX as usize {
                buf[(x as u16, y as u16)]
                    .set_symbol(panel_edge_glyph(cell.mask).encode_utf8(&mut [0; 4]))
                    .set_style(Style::default().fg(color));
            }
        }

        for node in &panel.nodes {
            let Some(&(x, y)) = node_positions.get(&node.task_id) else {
                continue;
            };
            if x > u16::MAX as usize || y > u16::MAX as usize {
                continue;
            }
            let node_style = panel_node_style(&node.status);
            let glyph = panel_node_glyph(node);
            let x = x as u16;
            let y = y as u16;
            buf[(x, y)]
                .set_symbol(glyph.encode_utf8(&mut [0; 4]))
                .set_style(node_style.add_modifier(Modifier::BOLD));

            let label = format!("{:02}", node.ordinal);
            let label_x = x.saturating_add(2);
            if label_x < inner.right() {
                for (offset, ch) in label.chars().enumerate() {
                    let cell_x = label_x.saturating_add(offset as u16);
                    if cell_x >= inner.right() {
                        break;
                    }
                    buf[(cell_x, y)]
                        .set_symbol(ch.encode_utf8(&mut [0; 4]))
                        .set_style(theme::text::muted());
                }
            }
        }

        let summary_y = inner.bottom().saturating_sub(2);
        let summary = format!(
            "● impl  ■ gate  run {}  fail {}",
            panel.running_count(),
            panel.failed_count()
        );
        for (offset, ch) in truncate_label(&summary, inner.width.saturating_sub(2) as usize)
            .chars()
            .enumerate()
        {
            let x = inner.x.saturating_add(1 + offset as u16);
            if x >= inner.right() {
                break;
            }
            let style = match ch {
                '●' | '■' => theme::text::primary().add_modifier(Modifier::BOLD),
                _ => theme::text::muted(),
            };
            buf[(x, summary_y)]
                .set_symbol(ch.encode_utf8(&mut [0; 4]))
                .set_style(style);
        }
    }
}

fn draw_horizontal_edge(
    cells: &mut BTreeMap<(usize, usize), DagPanelCell>,
    y: usize,
    start_x: usize,
    end_x: usize,
    animated: bool,
) {
    let (from, to) = if start_x <= end_x {
        (start_x, end_x)
    } else {
        (end_x, start_x)
    };
    for x in from..=to {
        let cell = cells.entry((x, y)).or_default();
        if x > from {
            cell.mask |= 0b1000;
        }
        if x < to {
            cell.mask |= 0b0010;
        }
        cell.animated |= animated;
    }
}

fn draw_vertical_edge(
    cells: &mut BTreeMap<(usize, usize), DagPanelCell>,
    x: usize,
    start_y: usize,
    end_y: usize,
    animated: bool,
) {
    let (from, to) = if start_y <= end_y {
        (start_y, end_y)
    } else {
        (end_y, start_y)
    };
    for y in from..=to {
        let cell = cells.entry((x, y)).or_default();
        if y > from {
            cell.mask |= 0b0001;
        }
        if y < to {
            cell.mask |= 0b0100;
        }
        cell.animated |= animated;
    }
}

fn panel_edge_glyph(mask: u8) -> char {
    match mask {
        0 => ' ',
        0b0010 | 0b1000 | 0b1010 => '─',
        0b0001 | 0b0100 | 0b0101 => '│',
        0b0110 => '┌',
        0b1100 => '┐',
        0b0011 => '└',
        0b1001 => '┘',
        0b0111 => '├',
        0b1101 => '┤',
        0b1110 => '┬',
        0b1011 => '┴',
        0b1111 => '┼',
        _ => '•',
    }
}

fn panel_node_glyph(node: &DagPanelNode) -> char {
    match (&node.kind, &node.status) {
        (TaskKind::Implementation, TaskNodeStatus::Completed) => '●',
        (TaskKind::Implementation, TaskNodeStatus::Running) => '◉',
        (TaskKind::Implementation, TaskNodeStatus::Failed) => '◍',
        (TaskKind::Implementation, TaskNodeStatus::Skipped) => '·',
        (TaskKind::Implementation, TaskNodeStatus::Pending) => '·',
        (TaskKind::Gate, TaskNodeStatus::Completed) => '■',
        (TaskKind::Gate, TaskNodeStatus::Running) => '▣',
        (TaskKind::Gate, TaskNodeStatus::Failed) => '▨',
        (TaskKind::Gate, TaskNodeStatus::Skipped) => '□',
        (TaskKind::Gate, TaskNodeStatus::Pending) => '□',
    }
}

fn panel_node_style(status: &TaskNodeStatus) -> Style {
    match status {
        TaskNodeStatus::Pending => theme::text::subtle(),
        TaskNodeStatus::Running => theme::interactive::in_progress(),
        TaskNodeStatus::Completed => theme::status::success(),
        TaskNodeStatus::Failed => theme::status::danger(),
        TaskNodeStatus::Skipped => theme::text::muted(),
    }
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }

    let mut truncated = String::new();
    for ch in text.chars().take(max_chars.saturating_sub(1)) {
        truncated.push(ch);
    }
    truncated.push('…');
    truncated
}

impl Default for ChatWidget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};
    use ratatui::{buffer::Buffer, layout::Rect};

    use super::ChatWidget;
    use crate::internal::ai::orchestrator::types::{
        ExecutionPlanSpec, TaskContract, TaskKind, TaskNodeStatus, TaskSpec,
    };

    fn row_text(buf: &Buffer, y: u16, width: u16) -> String {
        let mut out = String::new();
        for x in 0..width {
            out.push_str(buf[(x, y)].symbol());
        }
        out
    }

    fn make_task(title: &str, kind: TaskKind, dependencies: Vec<uuid::Uuid>) -> TaskSpec {
        let actor = ActorRef::agent("dag-panel-test").unwrap();
        let mut task = GitTask::new(actor, title, None).unwrap();
        for dependency in dependencies {
            task.add_dependency(dependency);
        }
        TaskSpec {
            step: git_internal::internal::object::plan::PlanStep::new(title),
            task,
            objective: title.to_string(),
            kind,
            gate_stage: None,
            owner_role: None,
            scope_in: vec![],
            scope_out: vec![],
            checks: vec![],
            contract: TaskContract::default(),
        }
    }

    fn sample_plan() -> ExecutionPlanSpec {
        let first = make_task(
            "Analyze repository structure",
            TaskKind::Implementation,
            vec![],
        );
        let second = make_task("Fast gate", TaskKind::Gate, vec![first.id()]);
        ExecutionPlanSpec {
            intent_spec_id: "intent-1".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![first, second],
            max_parallel: 1,
            checkpoints: vec![],
        }
    }

    #[test]
    fn dag_panel_uses_side_column_when_wide() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_plan());

        let (history, dag) = widget.split_chat_columns(Rect::new(0, 0, 120, 30));

        assert_eq!(history.width + dag.unwrap().width, 120);
        assert!(history.width < 120);
    }

    #[test]
    fn dag_panel_hides_when_narrow() {
        let mut widget = ChatWidget::new();
        widget.show_dag_panel(sample_plan());

        let (history, dag) = widget.split_chat_columns(Rect::new(0, 0, 80, 24));

        assert_eq!(history.width, 80);
        assert!(dag.is_none());
    }

    #[test]
    fn dag_panel_renders_graph_without_task_titles() {
        let plan = sample_plan();
        let first_task_id = plan.tasks[0].id();

        let mut widget = ChatWidget::new();
        widget.show_dag_panel(plan);
        widget.update_dag_task_status(first_task_id, TaskNodeStatus::Completed);
        widget.update_dag_progress(1, 2);

        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        widget.render_chat_area(area, &mut buf);

        let rendered = (0..area.height)
            .map(|y| row_text(&buf, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Workflow r1"));
        assert!(rendered.contains('●'));
        assert!(rendered.contains('□'));
        assert!(rendered.contains('│') || rendered.contains('─'));
        assert!(!rendered.contains("Analyze repository structure"));
        assert!(!rendered.contains("Fast gate"));
    }
}
