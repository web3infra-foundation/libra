//! Provider-neutral usage recording and display helpers (CEX-16).

pub mod format;
pub mod query;
pub mod recorder;

pub use format::{UsageDisplaySnapshot, format_usage_badge};
pub use query::{UsageAggregate, UsageQuery, UsageQueryFilter};
pub use recorder::{UsageContext, UsageRecorder};
