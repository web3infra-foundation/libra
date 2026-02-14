//! TUI module for Libra Code interactive interface.
//!
//! This module provides a terminal user interface built with ratatui for
//! interactive coding sessions with the AI agent.

mod app;
mod app_event;
mod bottom_pane;
mod chatwidget;
mod command_popup;
mod diff;
mod history_cell;
mod slash_command;
mod status_indicator;
mod terminal;

pub use app::{App, AppExitInfo, ExitReason};
pub use app_event::{AgentEvent, AgentStatus, AppEvent, ExitMode};
pub use command_popup::CommandPopup;
pub use diff::{DiffSummary, FileChange};
pub use history_cell::DiffHistoryCell;
pub use slash_command::{SlashCommand, get_commands_for_input, parse_command};
pub use status_indicator::StatusIndicator;
pub use terminal::{Tui, TuiEvent, init as tui_init, restore as tui_restore};
