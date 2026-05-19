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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_count_matches_table_size() {
        // INVARIANT: the modulo step in `spinner_frame` assumes the
        // SPINNER_FRAMES table is at least one entry. The animation
        // also assumes a 4-frame cycle. A silent change in either
        // direction would either deadlock the spinner on one frame
        // (1 entry) or rotate through a different pattern.
        assert_eq!(SPINNER_FRAMES.len(), 4);
        assert_eq!(SPINNER_FRAMES, ["│", "┐", "─", "└"]);
    }

    #[test]
    fn spinner_interval_is_one_hundred_milliseconds() {
        // INVARIANT: tied to the documented ~10 fps spinner cadence.
        // A change here desyncs animation from the doc.
        assert_eq!(SPINNER_INTERVAL_MS, 100);
    }

    #[test]
    fn default_is_idle_and_invisible() {
        let indicator = StatusIndicator::default();
        assert_eq!(indicator.status, AgentStatus::Idle);
        assert!(!indicator.is_visible());
    }

    #[test]
    fn new_records_the_supplied_status() {
        let indicator = StatusIndicator::new(AgentStatus::Thinking);
        assert_eq!(indicator.status, AgentStatus::Thinking);
        assert!(indicator.is_visible());
    }

    #[test]
    fn is_visible_returns_false_for_idle_only() {
        // INVARIANT: every non-Idle variant must reserve a row in the
        // layout. Even the modal "awaiting choice" variants count as
        // visible at this layer; the modal short-circuit lives inside
        // `render`, not here.
        assert!(!StatusIndicator::new(AgentStatus::Idle).is_visible());
        for status in [
            AgentStatus::Thinking,
            AgentStatus::Retrying,
            AgentStatus::ExecutingTool,
            AgentStatus::AwaitingUserInput,
            AgentStatus::AwaitingApproval,
            AgentStatus::AwaitingPostPlanChoice,
            AgentStatus::AwaitingNetworkPolicyChoice,
            AgentStatus::AwaitingIntentReviewChoice,
        ] {
            assert!(
                StatusIndicator::new(status).is_visible(),
                "{status:?} must report is_visible() == true"
            );
        }
    }

    #[test]
    fn update_status_to_same_status_is_noop_keeping_timer() {
        let mut indicator = StatusIndicator::new(AgentStatus::Thinking);
        let before = indicator.started_at;
        std::thread::sleep(Duration::from_millis(2));
        indicator.update_status(AgentStatus::Thinking);
        // INVARIANT: identical status must not reset the timer; the
        // elapsed counter should continue ticking through repeated
        // self-transitions emitted by the runtime.
        assert_eq!(indicator.started_at, before);
        assert_eq!(indicator.status, AgentStatus::Thinking);
    }

    #[test]
    fn update_status_idle_to_busy_resets_timer() {
        let mut indicator = StatusIndicator::default();
        let before = indicator.started_at;
        std::thread::sleep(Duration::from_millis(2));
        indicator.update_status(AgentStatus::Thinking);
        assert_eq!(indicator.status, AgentStatus::Thinking);
        assert!(
            indicator.started_at > before,
            "Idle → busy transition must reset started_at"
        );
    }

    #[test]
    fn update_status_busy_to_busy_keeps_timer_running() {
        // INVARIANT: total turn duration must survive intermediate
        // tool-call / retry state changes. Resetting the timer here
        // would make the UI feel like the agent restarted each step.
        let mut indicator = StatusIndicator::new(AgentStatus::Thinking);
        let before = indicator.started_at;
        std::thread::sleep(Duration::from_millis(2));
        indicator.update_status(AgentStatus::ExecutingTool);
        assert_eq!(indicator.status, AgentStatus::ExecutingTool);
        assert_eq!(
            indicator.started_at, before,
            "non-Idle → non-Idle transition must keep started_at"
        );
    }

    #[test]
    fn update_status_busy_to_idle_does_not_reset_timer() {
        // The reset rule only fires on Idle → non-Idle. A non-Idle →
        // Idle leg simply hides the widget; if the agent immediately
        // re-enters a busy state on the next event, the reset happens
        // then.
        let mut indicator = StatusIndicator::new(AgentStatus::ExecutingTool);
        let before = indicator.started_at;
        indicator.update_status(AgentStatus::Idle);
        assert_eq!(indicator.started_at, before);
        assert_eq!(indicator.status, AgentStatus::Idle);
        assert!(!indicator.is_visible());
    }

    /// Helper that lets the elapsed-time formatter be exercised at
    /// arbitrary durations without sleeping for hours.
    fn render_elapsed_at(secs: u64, nanos: u32) -> String {
        let indicator = StatusIndicator {
            started_at: Instant::now() - Duration::new(secs, nanos),
            status: AgentStatus::Thinking,
        };
        indicator.format_elapsed()
    }

    #[test]
    fn format_elapsed_under_one_minute_is_seconds_only() {
        assert_eq!(render_elapsed_at(0, 0), "0s");
        assert_eq!(render_elapsed_at(1, 0), "1s");
        assert_eq!(render_elapsed_at(42, 0), "42s");
        // Nanoseconds remain stripped because Duration::as_secs
        // truncates, matching the implementation.
        assert_eq!(render_elapsed_at(59, 999_000_000), "59s");
    }

    #[test]
    fn format_elapsed_between_one_minute_and_one_hour_is_m_ss() {
        assert_eq!(render_elapsed_at(60, 0), "1m 00s");
        assert_eq!(render_elapsed_at(61, 0), "1m 01s");
        assert_eq!(render_elapsed_at(125, 0), "2m 05s");
        assert_eq!(render_elapsed_at(3_599, 0), "59m 59s");
        // INVARIANT: seconds are zero-padded; a regression that
        // dropped the `{:02}` would print `1m 0s` instead of `1m 00s`.
        assert!(
            render_elapsed_at(60, 0).contains("00s"),
            "seconds field must be zero-padded"
        );
    }

    #[test]
    fn format_elapsed_one_hour_and_above_is_h_mm_ss() {
        assert_eq!(render_elapsed_at(3_600, 0), "1h 00m 00s");
        assert_eq!(render_elapsed_at(3_661, 0), "1h 01m 01s");
        assert_eq!(render_elapsed_at(7_265, 0), "2h 01m 05s");
        // Both fields must be zero-padded.
        assert!(
            render_elapsed_at(3_600, 0).contains("00m"),
            "minutes field must be zero-padded"
        );
    }

    #[test]
    fn spinner_frame_cycles_with_elapsed_time() {
        // INVARIANT: at +0ms we expect frame 0; at +200ms frame 2; at
        // +400ms back to frame 0 (cycles through 4).
        let frame_at = |ms: u64| {
            let indicator = StatusIndicator {
                started_at: Instant::now() - Duration::from_millis(ms),
                status: AgentStatus::Thinking,
            };
            indicator.spinner_frame().to_string()
        };
        // Use just-over the interval boundary to avoid Instant timing
        // jitter on slow CI.
        assert_eq!(frame_at(0), SPINNER_FRAMES[0]);
        assert_eq!(frame_at(110), SPINNER_FRAMES[1]);
        assert_eq!(frame_at(210), SPINNER_FRAMES[2]);
        assert_eq!(frame_at(310), SPINNER_FRAMES[3]);
        assert_eq!(
            frame_at(410),
            SPINNER_FRAMES[0],
            "must wrap back to frame 0"
        );
    }
}
