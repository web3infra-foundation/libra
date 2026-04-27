//! Status indicator widget shown above the composer while the agent is busy.
//!
//! Renders a single dim line of the form
//! `<spinner> <status>  <elapsed>  [Esc: Interrupt]` whenever the agent is in a
//! non-idle, non-modal state. The spinner advances via the elapsed wall clock
//! so it animates correctly even if the UI is not asked to redraw on every
//! frame; the redraw scheduler ([`super::terminal::TARGET_FRAME_INTERVAL`]) is
//! responsible for actually pulling new frames at ~60 FPS.

use std::time::{Duration, Instant};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::app_event::AgentStatus;

/// Four-frame ASCII spinner that rotates clockwise.
const SPINNER_FRAMES: [&str; 4] = ["│", "┐", "─", "└"];
/// Milliseconds per spinner frame; combined with [`Instant::elapsed`] this
/// yields a redraw-independent animation rate.
const SPINNER_INTERVAL_MS: u128 = 100;

/// Spinner + elapsed-time + hint widget rendered above the composer.
///
/// Tracks both the current [`AgentStatus`] and the wall-clock instant when the
/// agent first transitioned out of [`AgentStatus::Idle`]. The instant is used
/// to compute both the spinner phase and the elapsed-time display.
pub struct StatusIndicator {
    /// Timestamp of the most recent transition out of `Idle`. Used as the
    /// origin for both the spinner animation and the elapsed counter.
    started_at: Instant,
    /// Current agent status. When set to `Idle` the widget renders nothing.
    status: AgentStatus,
}

impl StatusIndicator {
    /// Construct a new indicator pre-populated with `status`.
    ///
    /// Functional scope: stamps `started_at` to `now` so that elapsed-time
    /// readings are zero at construction. Construction is the only place where
    /// the timer is unconditionally reset.
    pub fn new(status: AgentStatus) -> Self {
        Self {
            started_at: Instant::now(),
            status,
        }
    }

    /// Update the displayed status, resetting the elapsed timer when the agent
    /// transitions out of `Idle`.
    ///
    /// Functional scope: idempotent on no-op transitions (same status); resets
    /// `started_at` only on `Idle` to non-`Idle` transitions so a sustained
    /// busy session shows a continuous timer instead of restarting on every
    /// state change.
    ///
    /// Boundary conditions: transitions between two non-`Idle` statuses (e.g.
    /// `Thinking` to `ExecutingTool`) intentionally keep the existing timer
    /// to convey total turn duration, not per-state duration.
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

    /// Whether the widget would render anything for the current status.
    ///
    /// Functional scope: lets the layout reserve zero rows for the indicator
    /// while the agent is idle, giving the composer the full bottom area.
    pub fn is_visible(&self) -> bool {
        self.status != AgentStatus::Idle
    }

    /// Wall-clock duration since the most recent reset of `started_at`.
    fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Format the elapsed time as a compact human string.
    ///
    /// Boundary conditions:
    /// - Under one minute: `"<n>s"` (e.g. `"42s"`).
    /// - Under one hour: `"<m>m <SS>s"` with zero-padded seconds.
    /// - Otherwise: `"<h>h <MM>m <SS>s"` with both fields zero-padded.
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

    /// Pick the spinner character for the current frame based on elapsed time.
    ///
    /// Independent of the redraw cadence: even if the UI is asked to redraw
    /// rapidly the visible frame only advances every `SPINNER_INTERVAL_MS`.
    fn spinner_frame(&self) -> &str {
        let frame_idx = (self.elapsed().as_millis() / SPINNER_INTERVAL_MS) as usize;
        SPINNER_FRAMES[frame_idx % SPINNER_FRAMES.len()]
    }

    /// Render the indicator into `buf`.
    ///
    /// Functional scope: produces a single dim line containing the spinner,
    /// the human-readable status, the elapsed time, and the keybinding hint.
    /// Rendering is unconditionally skipped when the widget is invisible
    /// (idle) or the status is one of the modal "awaiting choice" variants —
    /// those render their own dedicated UIs in the bottom pane and would
    /// otherwise compete for vertical space.
    ///
    /// Boundary conditions: respects the supplied `area` exactly. If `area`
    /// has zero rows (e.g. the composer expanded), no painting occurs because
    /// `Paragraph::render` is a no-op on empty rectangles.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if !self.is_visible() {
            return;
        }

        // Pick a human label for the status. Modal "awaiting choice" states
        // are rendered by the bottom pane's own popups, so they short-circuit
        // here to avoid double-painting.
        let status_text = match self.status {
            AgentStatus::Thinking => "Thinking",
            AgentStatus::Retrying => "Retrying",
            AgentStatus::ExecutingTool => "Executing tool",
            AgentStatus::AwaitingUserInput => "Awaiting input",
            AgentStatus::AwaitingApproval => "Awaiting approval",
            AgentStatus::Idle
            | AgentStatus::AwaitingPostPlanChoice
            | AgentStatus::AwaitingNetworkPolicyChoice
            | AgentStatus::AwaitingIntentReviewChoice => return,
        };

        let spinner = self.spinner_frame();
        let elapsed = self.format_elapsed();

        // Layout: <spinner> <status>  <elapsed>  [Esc: Interrupt]
        let content = format!("{} {}  {}  [Esc: Interrupt]", spinner, status_text, elapsed);

        // Always rendered DIM so it sits below the transcript without
        // competing for visual weight.
        let style = Style::default().add_modifier(Modifier::DIM);

        let line = Line::styled(content, style);
        Paragraph::new(line).render(area, buf);
    }
}

impl Default for StatusIndicator {
    /// Construct an indicator in the `Idle` state with the timer pinned to now.
    /// `Default` is convenient for callers that build the App incrementally.
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            status: AgentStatus::Idle,
        }
    }
}
