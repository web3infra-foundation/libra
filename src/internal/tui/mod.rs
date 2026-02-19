//! TUI module for Libra Code interactive interface.
//!
//! This module provides a terminal user interface built with ratatui for
//! interactive coding sessions with the AI agent.

mod app;
mod app_event;
mod bottom_pane;
mod chatwidget;
mod diff;
mod history_cell;
mod terminal;

pub use app::{App, AppConfig, AppExitInfo, ExitReason};
pub use app_event::{AgentEvent, AgentStatus, AppEvent, ExitMode};
pub use diff::{DiffSummary, FileChange};
pub use history_cell::DiffHistoryCell;
pub use terminal::{Tui, TuiEvent, init as tui_init, restore as tui_restore};
