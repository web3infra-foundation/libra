//! Phase 0 Intent — formal write helpers.
//!
//! The Code UI Phase Workflow models Phase 0 as the **Intent** phase: a user
//! request is canonicalised into an [`IntentSpec`] and recorded as a draft
//! `Intent` revision in the AI object store. This module is the *formal
//! write* surface for that phase.
//!
//! # Design note
//!
//! Per [`docs/improvement/agent.md`](../../../../../docs/improvement/agent.md)
//! Part B Phase 0 plan, the long-term goal is for the Runtime to own the only
//! formal-write entry point for each phase. As a Wave 1B incremental step,
//! the helpers below are thin shims over the existing scattered persistence
//! logic in [`crate::internal::ai::intentspec::persistence`]; once Wave 1B
//! fully lands, downstream call sites
//! ([`crate::internal::ai::orchestrator::persistence::ExecutionAuditSession`],
//! `command::code`) will be redirected through these wrappers.
//!
//! The public API surface is intentionally minimal so the contract stays
//! stable even after the underlying call routes change.

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::internal::ai::{
    intentspec::{IntentSpec, persistence::persist_intentspec},
    mcp::server::LibraMcpServer,
};

/// Outcome of [`write_intent`]: the persisted intent revision id alongside a
/// reference back to the source [`IntentSpec`] so audit / observer code can
/// correlate the formal write with the request.
#[derive(Clone, Debug)]
pub struct IntentWriteOutcome {
    /// Identifier of the persisted Intent revision (the value that
    /// downstream Phase 1 / Phase 2 helpers reference when reading the
    /// intent back).
    pub intent_id: String,
    /// The original [`IntentSpec`] that was persisted. Kept verbatim so
    /// callers don't have to re-load the spec from storage for follow-up
    /// audit / observer events.
    pub source: IntentSpec,
}

/// Persist a new draft `Intent` revision as the **formal write** for Phase 0.
///
/// This is the entry point intended for Runtime callers; it delegates to
/// [`persist_intentspec`] today and will be the only sanctioned write path
/// once Wave 1B redirects existing call sites through this module.
///
/// # Returns
///
/// Wraps the persisted `intent_id` together with the original `spec` so
/// observers / audit sinks can record both without re-loading from storage.
///
/// # Errors
///
/// Returns the underlying `anyhow::Error` from `persist_intentspec` with the
/// added context `"Phase 0 write_intent"` so log scrapers can attribute the
/// failure to the formal-write layer.
pub async fn write_intent(
    spec: &IntentSpec,
    mcp_server: &Arc<LibraMcpServer>,
) -> Result<IntentWriteOutcome> {
    let intent_id = persist_intentspec(spec, mcp_server)
        .await
        .context("Phase 0 write_intent: persist_intentspec failed")?;

    Ok(IntentWriteOutcome {
        intent_id,
        source: spec.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::{
        DraftAcceptance, DraftIntent as DraftIntentBody, DraftRisk, IntentDraft, ResolveContext,
        RiskLevel, resolve_intentspec,
        types::{ChangeType, Objective, ObjectiveKind},
    };

    /// Build a minimal but real `IntentSpec` so the `IntentWriteOutcome`
    /// equality assertions exercise the actual `PartialEq` impl rather than
    /// a forced-default placeholder.
    fn sample_intent_spec() -> IntentSpec {
        resolve_intentspec(
            IntentDraft {
                intent: DraftIntentBody {
                    summary: "phase0 sample".to_string(),
                    problem_statement: "exercise outcome equality".to_string(),
                    change_type: ChangeType::Bugfix,
                    objectives: vec![Objective {
                        title: "test".to_string(),
                        kind: ObjectiveKind::Implementation,
                    }],
                    in_scope: vec!["src".to_string()],
                    out_of_scope: vec![],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["compiles".to_string()],
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                risk: DraftRisk {
                    rationale: "low".to_string(),
                    factors: vec![],
                    level: Some(RiskLevel::Low),
                },
            },
            RiskLevel::Low,
            ResolveContext {
                working_dir: "/tmp".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "phase0-test".to_string(),
            },
        )
    }

    /// `IntentWriteOutcome` carries both the persisted id and the original
    /// spec so observers don't have to re-load on the audit path.
    #[test]
    fn outcome_preserves_intent_id_and_source() {
        let spec = sample_intent_spec();
        let outcome = IntentWriteOutcome {
            intent_id: "intent-abc".to_string(),
            source: spec.clone(),
        };

        assert_eq!(outcome.intent_id, "intent-abc");
        assert_eq!(outcome.source, spec);
    }

    /// `IntentWriteOutcome` must derive `Clone` so audit handlers can keep a
    /// snapshot while the caller continues mutating the original spec.
    #[test]
    fn outcome_is_clone() {
        let outcome = IntentWriteOutcome {
            intent_id: "intent-xyz".to_string(),
            source: sample_intent_spec(),
        };
        let cloned = outcome.clone();
        assert_eq!(cloned.intent_id, outcome.intent_id);
        assert_eq!(cloned.source, outcome.source);
    }
}
