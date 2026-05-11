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
