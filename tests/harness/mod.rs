//! Cross-process test harness for Local TUI Automation Control.

mod code_session;
pub mod event_stream;
pub mod matrix;
mod scenario;

pub use code_session::{CodeSession, CodeSessionOptions};
pub use event_stream::{EventStream, SseEvent};
#[allow(unused_imports)]
pub use scenario::Scenario;
