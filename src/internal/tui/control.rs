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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TuiControlError {
    Busy,
    InteractionNotActive,
    UnsupportedInteractionKind,
    ControllerConflict,
    Internal(String),
}

impl TuiControlError {
    pub fn status(&self) -> u16 {
        match self {
            Self::Busy => 409,
            Self::InteractionNotActive => 409,
            Self::UnsupportedInteractionKind => 422,
            Self::ControllerConflict => 409,
            Self::Internal(_) => 500,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Busy => "SESSION_BUSY",
            Self::InteractionNotActive => "INTERACTION_NOT_ACTIVE",
            Self::UnsupportedInteractionKind => "UNSUPPORTED_INTERACTION_KIND",
            Self::ControllerConflict => "CONTROLLER_CONFLICT",
            Self::Internal(_) => "TUI_CONTROL_INTERNAL",
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
