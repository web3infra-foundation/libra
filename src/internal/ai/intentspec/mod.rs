//! IntentSpec subsystem: draft extraction, canonicalization, validation, repair,
//! persistence, and summary rendering for AI-driven work.
//!
//! Boundary: this module owns the "what should be done" contract. Execution planning,
//! workspace mutation, and final gate decisions live under `orchestrator`. End-to-end
//! coverage is in `tests/intent_flow_test.rs` and the AI validation decision tests.

pub mod canonical;
pub mod draft;
pub mod persistence;
pub mod profiles;
pub mod repair;
pub mod resolver;
pub mod review;
pub mod scope;
pub mod summary;
pub mod types;
pub mod validator;

pub use draft::{DraftAcceptance, DraftCheck, DraftIntent, DraftRisk, IntentDraft};
pub use persistence::persist_intentspec;
pub use repair::repair_intentspec;
pub use resolver::{ResolveContext, resolve_intentspec};
pub use review::build_intentspec_review;
pub use scope::{effective_forbidden_scope, effective_write_scope};
pub use summary::render_summary;
pub use types::{IntentSpec, RiskLevel};
pub use validator::{ValidationIssue, validate_intentspec};
