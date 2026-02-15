use std::time::{Duration, Instant};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::app_event::AgentStatus;

const SPINNER_FRAMES: [&str; 4] = ["│", "┐", "─", "└"];
const SPINNER_INTERVAL_MS: u128 = 100;

pub struct StatusIndicator {
    started_at: Instant,
    status: AgentStatus,
}

impl StatusIndicator {
    pub fn new(status: AgentStatus) -> Self {
        Self {
            started_at: Instant::now(),
            status,
        }
    }

    pub fn update_status(&mut self, status: AgentStatus) {
        // No-op if the status is unchanged.
        if self.status == status {
            return;
        }
        // When transitioning from Idle to a non-idle status, reset the timer.
        if self.status == AgentStatus::Idle && status != AgentStatus::Idle {
            self.started_at = Instant::now();
        }
        self.status = status;
    }

    pub fn is_visible(&self) -> bool {
        self.status != AgentStatus::Idle
    }

    fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    fn format_elapsed(&self) -> String {
        let elapsed = self.elapsed();
        let secs = elapsed.as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            let mins = secs / 60;
            let secs = secs % 60;
            format!("{}m {:02}s", mins, secs)
        } else {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            let secs = secs % 60;
            format!("{}h {:02}m {:02}s", hours, mins, secs)
        }
    }

    fn spinner_frame(&self) -> &str {
        let frame_idx = (self.elapsed().as_millis() / SPINNER_INTERVAL_MS) as usize;
        SPINNER_FRAMES[frame_idx % SPINNER_FRAMES.len()]
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if !self.is_visible() {
            return;
        }

        let status_text = match self.status {
            AgentStatus::Thinking => "Thinking",
            AgentStatus::ExecutingTool => "Executing tool",
            AgentStatus::Idle => return,
        };

        let spinner = self.spinner_frame();
        let elapsed = self.format_elapsed();

        let content = format!("{} {}  {}  [Esc: Interrupt]", spinner, status_text, elapsed);

        let style = Style::default().fg(Color::DarkGray);

        let line = Line::styled(content, style);
        Paragraph::new(line).render(area, buf);
    }
}

impl Default for StatusIndicator {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            status: AgentStatus::Idle,
        }
    }
}
