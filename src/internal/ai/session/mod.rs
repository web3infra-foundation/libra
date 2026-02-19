//! Session persistence for saving and restoring conversation state.
//!
//! Sessions capture conversation history, working directory, context mode,
//! and metadata. They are stored as JSON files in `.libra/sessions/`.
//!
//! ## Usage
//!
//! ```no_run
//! use libra::internal::ai::session::{SessionState, SessionStore};
//!
//! // Create and populate a session
//! let mut session = SessionState::new("/path/to/project");
//! session.add_user_message("implement auth");
//! session.add_assistant_message("I'll implement authentication...");
//!
//! // Save to disk
//! let store = SessionStore::new(std::path::Path::new("/path/to/project"));
//! store.save(&session).unwrap();
//!
//! // Restore later
//! let restored = store.load_latest().unwrap();
//! ```

pub mod state;
pub mod store;

pub use state::{SessionId, SessionMessage, SessionState};
pub use store::{SessionInfo, SessionStore};
