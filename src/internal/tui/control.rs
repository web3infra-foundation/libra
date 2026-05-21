//! TUI automation control command channel.
//!
//! `TuiControlCommand` is deliberately independent of `AppEvent`. `AppEvent` carries
//! turn-scoped events (each variant exposes a `turn_id`), whereas automation
//! respond / cancel / reclaim are control-plane commands that may span turns or
//! occur when no turn is active. Mixing them would break the turn-scoped invariant
//! and complicate the App event bus.
//!
//! The App main loop consumes these commands through an additional branch in its
//! `tokio::select!` (`code_control_rx.recv()`). Each command carries a `oneshot`
//! ack so the HTTP handler can await acceptance or rejection (e.g. `Busy`,
//! `InteractionNotActive`) without blocking the App event loop. The default ack
//! timeout in the adapter is 30 seconds; the App must send the ack before that
//! deadline or the automation client receives a timeout error.

use std::fmt;

use tokio::sync::oneshot;

use crate::internal::ai::web::code_ui::CodeUiInteractionResponse;

pub enum TuiControlCommand {
    SubmitMessage {
        text: String,
        ack: oneshot::Sender<Result<(), TuiControlError>>,
    },
    RespondInteraction {
        interaction_id: String,
        response: CodeUiInteractionResponse,
        ack: oneshot::Sender<Result<(), TuiControlError>>,
    },
    CancelCurrentTurn {
        ack: oneshot::Sender<Result<(), TuiControlError>>,
    },
    ReclaimController {
        ack: oneshot::Sender<Result<(), TuiControlError>>,
    },
    /// `goal.start { objective }` — create an active Goal in the
    /// session, mirroring `/goal start <objective>` (OC-Phase 6
    /// P6.6). The acknowledgement carries the rendered status of
    /// the freshly created Goal so the Code Control client can
    /// surface the goal id without a follow-up `goal.status`.
    GoalStart {
        objective: String,
        ack: oneshot::Sender<Result<String, TuiControlError>>,
    },
    /// `goal.status` — render the active Goal's snapshot. The
    /// acknowledgement carries the formatted multi-line summary
    /// (or an `InteractionNotActive`-equivalent status if no Goal
    /// is in flight). Read-only; no controller-token required at
    /// the HTTP layer (loopback observer mode).
    GoalStatus {
        ack: oneshot::Sender<Result<String, TuiControlError>>,
    },
    /// `goal.cancel { reason }` — explicit cancellation. Mirrors
    /// `/goal cancel <reason>` and emits `GoalEvent::Cancelled`
    /// into the session's event log.
    GoalCancel {
        reason: String,
        ack: oneshot::Sender<Result<String, TuiControlError>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TuiControlError {
    Busy,
    InteractionNotActive,
    UnsupportedInteractionKind,
    ControllerConflict,
    Internal(String),
    /// `goal.start` was called while a Goal is already active in
    /// this session. The user / automation must `goal.cancel` (or
    /// wait for completion, once the supervisor lands) before
    /// starting a new one (OC-Phase 6 P6.6).
    GoalAlreadyActive,
    /// `goal.status` / `goal.cancel` was called when no Goal is
    /// active. Distinct from `InteractionNotActive` (which is
    /// scoped to pending tool-interaction prompts).
    GoalNotActive,
    /// Goal objective failed `GoalSpec`'s shape rules at the
    /// HTTP boundary (empty / oversized). The wire message
    /// repeats the underlying `GoalSpecError` for client logs.
    GoalInvalidObjective(String),
}

impl TuiControlError {
    pub fn status(&self) -> u16 {
        match self {
            Self::Busy => 409,
            Self::InteractionNotActive => 409,
            Self::UnsupportedInteractionKind => 422,
            Self::ControllerConflict => 409,
            Self::Internal(_) => 500,
            Self::GoalAlreadyActive => 409,
            Self::GoalNotActive => 409,
            Self::GoalInvalidObjective(_) => 422,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Busy => "SESSION_BUSY",
            Self::InteractionNotActive => "INTERACTION_NOT_ACTIVE",
            Self::UnsupportedInteractionKind => "UNSUPPORTED_INTERACTION_KIND",
            Self::ControllerConflict => "CONTROLLER_CONFLICT",
            Self::Internal(_) => "TUI_CONTROL_INTERNAL",
            Self::GoalAlreadyActive => "GOAL_ALREADY_ACTIVE",
            Self::GoalNotActive => "GOAL_NOT_ACTIVE",
            Self::GoalInvalidObjective(_) => "GOAL_INVALID_OBJECTIVE",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Busy => "The TUI session is busy and cannot accept a new message".to_string(),
            Self::InteractionNotActive => {
                "The requested interaction is not currently pending".to_string()
            }
            Self::UnsupportedInteractionKind => {
                "This interaction kind cannot be answered by automation".to_string()
            }
            Self::ControllerConflict => {
                "The local TUI controller is not reclaimable in this session".to_string()
            }
            Self::Internal(message) => message.clone(),
            Self::GoalAlreadyActive => {
                "A Goal is already active in this session — cancel it first".to_string()
            }
            Self::GoalNotActive => {
                "No Goal is active in this session — call goal.start first".to_string()
            }
            Self::GoalInvalidObjective(detail) => {
                format!("Goal objective failed validation: {detail}")
            }
        }
    }
}

impl fmt::Display for TuiControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for TuiControlError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelSource {
    Esc,
    SlashQuit,
    Automation,
}

#[cfg(test)]
mod tests {
    use super::TuiControlError;

    #[test]
    fn tui_control_error_display_pins_each_variant() {
        assert_eq!(
            TuiControlError::Busy.to_string(),
            "The TUI session is busy and cannot accept a new message",
        );
        assert_eq!(
            TuiControlError::InteractionNotActive.to_string(),
            "The requested interaction is not currently pending",
        );
        assert_eq!(
            TuiControlError::UnsupportedInteractionKind.to_string(),
            "This interaction kind cannot be answered by automation",
        );
        assert_eq!(
            TuiControlError::ControllerConflict.to_string(),
            "The local TUI controller is not reclaimable in this session",
        );
        assert_eq!(
            TuiControlError::Internal("downstream blew up".to_string()).to_string(),
            "downstream blew up",
        );
        assert_eq!(
            TuiControlError::GoalAlreadyActive.to_string(),
            "A Goal is already active in this session — cancel it first",
        );
        assert_eq!(
            TuiControlError::GoalNotActive.to_string(),
            "No Goal is active in this session — call goal.start first",
        );
        assert_eq!(
            TuiControlError::GoalInvalidObjective("empty objective".to_string()).to_string(),
            "Goal objective failed validation: empty objective",
        );
    }

    /// Pins the HTTP `status()` mapping for every variant. The wire
    /// contract surface includes both the human message (pinned in
    /// `tui_control_error_display_pins_each_variant`) and the status
    /// code — a future refactor that flips a 409 to a 500 (or adds a
    /// new variant defaulting to 500 by accident) would silently
    /// change client retry / error-classification behaviour. Listing
    /// every variant here keeps the parallel surface honest.
    #[test]
    fn tui_control_error_status_pins_each_variant() {
        assert_eq!(TuiControlError::Busy.status(), 409);
        assert_eq!(TuiControlError::InteractionNotActive.status(), 409);
        assert_eq!(TuiControlError::UnsupportedInteractionKind.status(), 422);
        assert_eq!(TuiControlError::ControllerConflict.status(), 409);
        assert_eq!(
            TuiControlError::Internal("ignored".to_string()).status(),
            500,
        );
        assert_eq!(TuiControlError::GoalAlreadyActive.status(), 409);
        assert_eq!(TuiControlError::GoalNotActive.status(), 409);
        assert_eq!(
            TuiControlError::GoalInvalidObjective("ignored".to_string()).status(),
            422,
        );
    }

    /// Pins the stable `code()` mapping for every variant. Clients
    /// (HTTP and automation) key off these SCREAMING_SNAKE_CASE codes
    /// rather than parsing the human message; a typo or renaming
    /// `SESSION_BUSY` → `BUSY_SESSION` would silently break every
    /// consumer that branches on the code. Pinning every variant
    /// turns a rename into a test failure.
    #[test]
    fn tui_control_error_code_pins_each_variant() {
        assert_eq!(TuiControlError::Busy.code(), "SESSION_BUSY");
        assert_eq!(
            TuiControlError::InteractionNotActive.code(),
            "INTERACTION_NOT_ACTIVE",
        );
        assert_eq!(
            TuiControlError::UnsupportedInteractionKind.code(),
            "UNSUPPORTED_INTERACTION_KIND",
        );
        assert_eq!(
            TuiControlError::ControllerConflict.code(),
            "CONTROLLER_CONFLICT",
        );
        assert_eq!(
            TuiControlError::Internal("ignored".to_string()).code(),
            "TUI_CONTROL_INTERNAL",
        );
        assert_eq!(
            TuiControlError::GoalAlreadyActive.code(),
            "GOAL_ALREADY_ACTIVE",
        );
        assert_eq!(TuiControlError::GoalNotActive.code(), "GOAL_NOT_ACTIVE");
        assert_eq!(
            TuiControlError::GoalInvalidObjective("ignored".to_string()).code(),
            "GOAL_INVALID_OBJECTIVE",
        );
    }
}
