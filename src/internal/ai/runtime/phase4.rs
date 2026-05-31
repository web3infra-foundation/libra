//! Phase 4 risk aggregation, decision proposals, and derived-record persistence.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::internal::{
    ai::{
        runtime::{
            contracts::FinalDecisionVerdict,
            derived_records::ensure_runtime_thread,
            phase3::{
                ValidationReport, ValidationStatus, bool_to_row, deserialize_summary, parse_uuid,
                serialize_summary, timestamp_from_row,
            },
        },
        session::jsonl::{AiArtifactEvent, SessionEvent, SessionJsonlStore},
    },
    model::{ai_decision_proposal, ai_final_decision, ai_risk_score_breakdown},
};

pub const DEFAULT_DECISION_POLICY_VERSION: &str = "decision:v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionPolicy {
    pub policy_version: String,
    pub auto_accept_max_score: u8,
}

impl Default for DecisionPolicy {
    fn default() -> Self {
        Self {
            policy_version: DEFAULT_DECISION_POLICY_VERSION.to_string(),
            auto_accept_max_score: 30,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskScoreSummary {
    pub score: u8,
    #[serde(default)]
    pub reasons: Vec<String>,
    pub validation_status: ValidationStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskScoreBreakdown {
    pub breakdown_id: Uuid,
    pub thread_id: Uuid,
    pub validation_report_id: Option<Uuid>,
    pub policy_version: String,
    pub stale: bool,
    pub is_latest: bool,
    pub summary: RiskScoreSummary,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionProposalRoute {
    AutoAccept,
    HumanReview,
    RequestChanges,
    Abandon,
}

impl DecisionProposalRoute {
    /// `true` only for [`AutoAccept`](Self::AutoAccept) — the path that
    /// bypasses human review entirely.
    pub fn is_auto_accept(self) -> bool {
        matches!(self, DecisionProposalRoute::AutoAccept)
    }

    /// `true` when the route requires human review before the verdict
    /// is committed. `RequestChanges` is also included here because a
    /// rejection must surface to a human before the loop continues.
    pub fn requires_human_review(self) -> bool {
        matches!(
            self,
            DecisionProposalRoute::HumanReview
                | DecisionProposalRoute::RequestChanges
                | DecisionProposalRoute::Abandon
        )
    }

    /// Stable lower-snake-case identifier matching the
    /// `#[serde(rename_all = "snake_case")]` tag values, so audit
    /// pipelines can stringify a `DecisionProposalRoute` without
    /// reaching for `serde_json::to_value`.
    pub fn variant_name(self) -> &'static str {
        match self {
            DecisionProposalRoute::AutoAccept => "auto_accept",
            DecisionProposalRoute::HumanReview => "human_review",
            DecisionProposalRoute::RequestChanges => "request_changes",
            DecisionProposalRoute::Abandon => "abandon",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionProposalSummary {
    pub route: DecisionProposalRoute,
    pub proposed_verdict: FinalDecisionVerdict,
    pub risk_score: u8,
    pub requires_human_review: bool,
    #[serde(default)]
    pub rationale: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionProposal {
    pub proposal_id: Uuid,
    pub thread_id: Uuid,
    pub validation_report_id: Option<Uuid>,
    pub risk_score_breakdown_id: Option<Uuid>,
    pub policy_version: String,
    pub stale: bool,
    pub is_latest: bool,
    pub summary: DecisionProposalSummary,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DecisionProposal {
    /// Convenience: `true` when this proposal recommends the
    /// [`AutoAccept`](DecisionProposalRoute::AutoAccept) route — i.e.
    /// the loop can commit without a human gate. Delegates to
    /// [`DecisionProposalRoute::is_auto_accept`].
    pub fn is_auto_accept(&self) -> bool {
        self.summary.route.is_auto_accept()
    }
}

pub fn aggregate_risk_score(
    report: &ValidationReport,
    policy: &DecisionPolicy,
) -> RiskScoreBreakdown {
    let mut reasons = Vec::new();
    let score = match report.summary.status {
        ValidationStatus::Passed => {
            reasons.push("validation passed".to_string());
            20
        }
        ValidationStatus::BlockingFailed => {
            reasons.push("validation has blocking failures".to_string());
            75
        }
        ValidationStatus::InfrastructureFailed => {
            reasons.push("validator infrastructure failed".to_string());
            90
        }
    };
    let now = Utc::now();
    RiskScoreBreakdown {
        breakdown_id: Uuid::new_v4(),
        thread_id: report.thread_id,
        validation_report_id: Some(report.report_id),
        policy_version: policy.policy_version.clone(),
        stale: report.stale,
        is_latest: true,
        summary: RiskScoreSummary {
            score,
            reasons,
            validation_status: report.summary.status,
        },
        created_at: now,
        updated_at: now,
    }
}

pub fn build_decision_proposal(
    report: &ValidationReport,
    risk: &RiskScoreBreakdown,
    policy: &DecisionPolicy,
) -> DecisionProposal {
    let (route, proposed_verdict, requires_human_review, mut rationale) =
        match report.summary.status {
            ValidationStatus::Passed if risk.summary.score <= policy.auto_accept_max_score => (
                DecisionProposalRoute::AutoAccept,
                FinalDecisionVerdict::Accepted,
                false,
                vec!["risk score is within automatic acceptance threshold".to_string()],
            ),
            ValidationStatus::Passed => (
                DecisionProposalRoute::HumanReview,
                FinalDecisionVerdict::Accepted,
                true,
                vec!["validation passed but risk score requires review".to_string()],
            ),
            ValidationStatus::BlockingFailed => (
                DecisionProposalRoute::RequestChanges,
                FinalDecisionVerdict::Rejected,
                true,
                vec!["blocking validation failure requires changes".to_string()],
            ),
            ValidationStatus::InfrastructureFailed => (
                DecisionProposalRoute::HumanReview,
                FinalDecisionVerdict::Abandon,
                true,
                vec!["validator infrastructure failed; human review required".to_string()],
            ),
        };
    rationale.extend(risk.summary.reasons.iter().cloned());
    let now = Utc::now();
    DecisionProposal {
        proposal_id: Uuid::new_v4(),
        thread_id: report.thread_id,
        validation_report_id: Some(report.report_id),
        risk_score_breakdown_id: Some(risk.breakdown_id),
        policy_version: policy.policy_version.clone(),
        stale: report.stale || risk.stale,
        is_latest: true,
        summary: DecisionProposalSummary {
            route,
            proposed_verdict,
            risk_score: risk.summary.score,
            requires_human_review,
            rationale,
        },
        created_at: now,
        updated_at: now,
    }
}

#[derive(Clone)]
pub struct DecisionProposalStore {
    db: DatabaseConnection,
}

impl DecisionProposalStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn write_latest(
        &self,
        risk: &RiskScoreBreakdown,
        proposal: &DecisionProposal,
    ) -> Result<()> {
        let txn = self
            .db
            .begin()
            .await
            .context("Failed to start decision proposal transaction")?;

        if risk.thread_id != proposal.thread_id {
            bail!(
                "Risk score thread {} does not match decision proposal thread {}",
                risk.thread_id,
                proposal.thread_id
            );
        }
        ensure_runtime_thread(&txn, proposal.thread_id).await?;

        ai_risk_score_breakdown::Entity::update_many()
            .col_expr(ai_risk_score_breakdown::Column::IsLatest, Expr::value(0))
            .filter(ai_risk_score_breakdown::Column::ThreadId.eq(risk.thread_id.to_string()))
            .exec(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to clear previous latest risk score for thread {}",
                    risk.thread_id
                )
            })?;
        ai_decision_proposal::Entity::update_many()
            .col_expr(ai_decision_proposal::Column::IsLatest, Expr::value(0))
            .filter(ai_decision_proposal::Column::ThreadId.eq(proposal.thread_id.to_string()))
            .exec(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to clear previous latest decision proposal for thread {}",
                    proposal.thread_id
                )
            })?;

        risk_to_active_model(risk)?
            .insert(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to insert risk score {} for thread {}",
                    risk.breakdown_id, risk.thread_id
                )
            })?;
        proposal_to_active_model(proposal)?
            .insert(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to insert decision proposal {} for thread {}",
                    proposal.proposal_id, proposal.thread_id
                )
            })?;

        txn.commit()
            .await
            .context("Failed to commit decision proposal transaction")?;
        Ok(())
    }

    pub async fn write_latest_with_session_mirror(
        &self,
        risk: &RiskScoreBreakdown,
        proposal: &DecisionProposal,
        session_store: &SessionJsonlStore,
    ) -> Result<()> {
        self.write_latest(risk, proposal).await?;
        append_decision_session_mirror(session_store, risk, proposal)?;
        Ok(())
    }

    pub async fn load_latest_risk(&self, thread_id: Uuid) -> Result<Option<RiskScoreBreakdown>> {
        ai_risk_score_breakdown::Entity::find()
            .filter(ai_risk_score_breakdown::Column::ThreadId.eq(thread_id.to_string()))
            .filter(ai_risk_score_breakdown::Column::IsLatest.eq(1))
            .order_by_desc(ai_risk_score_breakdown::Column::CreatedAt)
            .one(&self.db)
            .await
            .with_context(|| format!("Failed to load latest risk score for {thread_id}"))?
            .map(risk_from_model)
            .transpose()
    }

    pub async fn load_latest_proposal(&self, thread_id: Uuid) -> Result<Option<DecisionProposal>> {
        ai_decision_proposal::Entity::find()
            .filter(ai_decision_proposal::Column::ThreadId.eq(thread_id.to_string()))
            .filter(ai_decision_proposal::Column::IsLatest.eq(1))
            .order_by_desc(ai_decision_proposal::Column::CreatedAt)
            .one(&self.db)
            .await
            .with_context(|| format!("Failed to load latest decision proposal for {thread_id}"))?
            .map(proposal_from_model)
            .transpose()
    }
}

pub fn append_decision_session_mirror(
    session_store: &SessionJsonlStore,
    risk: &RiskScoreBreakdown,
    proposal: &DecisionProposal,
) -> Result<()> {
    if risk.thread_id != proposal.thread_id {
        bail!(
            "Risk score thread {} does not match decision proposal thread {}",
            risk.thread_id,
            proposal.thread_id
        );
    }

    let risk_event = SessionEvent::ai_artifact(risk_score_artifact_event(risk)?);
    session_store.append(&risk_event).with_context(|| {
        format!(
            "Failed to append risk score {} session artifact mirror for thread {} to {}",
            risk.breakdown_id,
            risk.thread_id,
            session_store.events_path().display()
        )
    })?;

    let proposal_event = SessionEvent::ai_artifact(decision_proposal_artifact_event(proposal)?);
    session_store.append(&proposal_event).with_context(|| {
        format!(
            "Failed to append decision proposal {} session artifact mirror for thread {} to {}",
            proposal.proposal_id,
            proposal.thread_id,
            session_store.events_path().display()
        )
    })
}

pub fn risk_score_artifact_event(risk: &RiskScoreBreakdown) -> Result<AiArtifactEvent> {
    Ok(AiArtifactEvent {
        event_id: Uuid::new_v4(),
        recorded_at: Utc::now(),
        thread_id: risk.thread_id,
        artifact_kind: "risk_score_breakdown".to_string(),
        artifact_id: Some(risk.breakdown_id.to_string()),
        payload: serde_json::to_value(risk).with_context(|| {
            format!(
                "Failed to serialize risk score {} for session artifact mirror",
                risk.breakdown_id
            )
        })?,
    })
}

pub fn decision_proposal_artifact_event(proposal: &DecisionProposal) -> Result<AiArtifactEvent> {
    Ok(AiArtifactEvent {
        event_id: Uuid::new_v4(),
        recorded_at: Utc::now(),
        thread_id: proposal.thread_id,
        artifact_kind: "decision_proposal".to_string(),
        artifact_id: Some(proposal.proposal_id.to_string()),
        payload: serde_json::to_value(proposal).with_context(|| {
            format!(
                "Failed to serialize decision proposal {} for session artifact mirror",
                proposal.proposal_id
            )
        })?,
    })
}

fn risk_to_active_model(risk: &RiskScoreBreakdown) -> Result<ai_risk_score_breakdown::ActiveModel> {
    Ok(ai_risk_score_breakdown::ActiveModel {
        breakdown_id: Set(risk.breakdown_id.to_string()),
        thread_id: Set(risk.thread_id.to_string()),
        validation_report_id: Set(risk.validation_report_id.map(|id| id.to_string())),
        policy_version: Set(risk.policy_version.clone()),
        stale: Set(bool_to_row(risk.stale)),
        is_latest: Set(bool_to_row(risk.is_latest)),
        summary_json: Set(serialize_summary(&risk.summary, "risk score summary")?),
        created_at: Set(risk.created_at.timestamp()),
        updated_at: Set(risk.updated_at.timestamp()),
    })
}

fn proposal_to_active_model(
    proposal: &DecisionProposal,
) -> Result<ai_decision_proposal::ActiveModel> {
    Ok(ai_decision_proposal::ActiveModel {
        proposal_id: Set(proposal.proposal_id.to_string()),
        thread_id: Set(proposal.thread_id.to_string()),
        validation_report_id: Set(proposal.validation_report_id.map(|id| id.to_string())),
        risk_score_breakdown_id: Set(proposal.risk_score_breakdown_id.map(|id| id.to_string())),
        policy_version: Set(proposal.policy_version.clone()),
        stale: Set(bool_to_row(proposal.stale)),
        is_latest: Set(bool_to_row(proposal.is_latest)),
        summary_json: Set(serialize_summary(
            &proposal.summary,
            "decision proposal summary",
        )?),
        created_at: Set(proposal.created_at.timestamp()),
        updated_at: Set(proposal.updated_at.timestamp()),
    })
}

fn risk_from_model(row: ai_risk_score_breakdown::Model) -> Result<RiskScoreBreakdown> {
    Ok(RiskScoreBreakdown {
        breakdown_id: parse_uuid(&row.breakdown_id, "risk breakdown_id")?,
        thread_id: parse_uuid(&row.thread_id, "risk thread_id")?,
        validation_report_id: row
            .validation_report_id
            .as_deref()
            .map(|raw| parse_uuid(raw, "risk validation_report_id"))
            .transpose()?,
        policy_version: row.policy_version,
        stale: row.stale != 0,
        is_latest: row.is_latest != 0,
        summary: deserialize_summary(&row.summary_json, "risk score summary")?,
        created_at: timestamp_from_row(row.created_at, "risk created_at")?,
        updated_at: timestamp_from_row(row.updated_at, "risk updated_at")?,
    })
}

fn proposal_from_model(row: ai_decision_proposal::Model) -> Result<DecisionProposal> {
    Ok(DecisionProposal {
        proposal_id: parse_uuid(&row.proposal_id, "decision proposal_id")?,
        thread_id: parse_uuid(&row.thread_id, "decision thread_id")?,
        validation_report_id: row
            .validation_report_id
            .as_deref()
            .map(|raw| parse_uuid(raw, "decision validation_report_id"))
            .transpose()?,
        risk_score_breakdown_id: row
            .risk_score_breakdown_id
            .as_deref()
            .map(|raw| parse_uuid(raw, "decision risk_score_breakdown_id"))
            .transpose()?,
        policy_version: row.policy_version,
        stale: row.stale != 0,
        is_latest: row.is_latest != 0,
        summary: deserialize_summary(&row.summary_json, "decision proposal summary")?,
        created_at: timestamp_from_row(row.created_at, "decision created_at")?,
        updated_at: timestamp_from_row(row.updated_at, "decision updated_at")?,
    })
}

// ---------------------------------------------------------------------------
// Phase 4 completion: the formal final `Decision` artifact.
//
// Terminal link in the ValidationReport -> RiskScoreBreakdown ->
// DecisionProposal -> Decision chain. A `DecisionProposal` recommends a
// route + verdict; when that route is `AutoAccept` (no human gate), the
// runtime finalises it into a `FinalDecision` recording the resolved verdict.
// Human-gated routes (HumanReview / RequestChanges) are finalised later by the
// CEX-S2-13 human-gated merge flow, which owns the approval interaction.
// ---------------------------------------------------------------------------

/// Richer detail persisted alongside a [`FinalDecision`] in `summary_json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalDecisionSummary {
    /// The proposal route that produced this decision (e.g. `AutoAccept`).
    pub route: DecisionProposalRoute,
    /// Aggregate risk score carried over from the proposal.
    pub risk_score: u8,
    /// Human-readable rationale lines carried over from the proposal.
    #[serde(default)]
    pub rationale: Vec<String>,
}

/// The runtime's formal, persisted final decision for a thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalDecision {
    pub decision_id: Uuid,
    pub thread_id: Uuid,
    pub decision_proposal_id: Option<Uuid>,
    pub validation_report_id: Option<Uuid>,
    pub policy_version: String,
    pub verdict: FinalDecisionVerdict,
    pub stale: bool,
    pub is_latest: bool,
    pub summary: FinalDecisionSummary,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl FinalDecision {
    /// Finalise an `AutoAccept` [`DecisionProposal`] into a [`FinalDecision`].
    ///
    /// Returns `None` when the proposal must not be auto-finalised:
    /// - its route is not [`AutoAccept`](DecisionProposalRoute::AutoAccept) —
    ///   human-gated routes resolve through the CEX-S2-13 human-gated merge
    ///   flow; or
    /// - the proposal is `stale`. The Phase-4 projection-freshness contract
    ///   (`docs/improvement/agent.md`, `ProjectionFreshness` table) forbids
    ///   writing a final `Decision` from a stale `ValidationReport` /
    ///   `RiskScoreBreakdown` / `DecisionProposal`; such a proposal must be
    ///   replayed / recomputed (or escalated to human review) first, never
    ///   silently promoted to a fresh terminal decision.
    ///
    /// `now` is supplied by the caller so the timestamps are testable.
    pub fn finalize_auto_accept(proposal: &DecisionProposal, now: DateTime<Utc>) -> Option<Self> {
        if !proposal.summary.route.is_auto_accept() || proposal.stale {
            return None;
        }
        Some(Self {
            decision_id: Uuid::new_v4(),
            thread_id: proposal.thread_id,
            decision_proposal_id: Some(proposal.proposal_id),
            validation_report_id: proposal.validation_report_id,
            policy_version: proposal.policy_version.clone(),
            verdict: proposal.summary.proposed_verdict.clone(),
            stale: false,
            is_latest: true,
            summary: FinalDecisionSummary {
                route: proposal.summary.route,
                risk_score: proposal.summary.risk_score,
                rationale: proposal.summary.rationale.clone(),
            },
            created_at: now,
            updated_at: now,
        })
    }
}

/// Persistence for [`FinalDecision`], mirroring [`DecisionProposalStore`]'s
/// per-thread latest-pointer pattern.
#[derive(Clone)]
pub struct FinalDecisionStore {
    db: DatabaseConnection,
}

impl FinalDecisionStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Insert `decision` as the thread's latest final decision, clearing the
    /// previous `is_latest` flag in the same transaction so the partial unique
    /// index (`is_latest = 1`) holds.
    pub async fn write_latest(&self, decision: &FinalDecision) -> Result<()> {
        let txn = self
            .db
            .begin()
            .await
            .context("Failed to start final decision transaction")?;

        ensure_runtime_thread(&txn, decision.thread_id).await?;

        ai_final_decision::Entity::update_many()
            .col_expr(ai_final_decision::Column::IsLatest, Expr::value(0))
            .filter(ai_final_decision::Column::ThreadId.eq(decision.thread_id.to_string()))
            .exec(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to clear previous latest final decision for thread {}",
                    decision.thread_id
                )
            })?;

        final_decision_to_active_model(decision)?
            .insert(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to insert final decision {} for thread {}",
                    decision.decision_id, decision.thread_id
                )
            })?;

        txn.commit()
            .await
            .context("Failed to commit final decision transaction")?;
        Ok(())
    }

    /// Persist `decision` and mirror it to the session JSONL stream as an
    /// `ai_artifact` event (kind `final_decision`), matching how
    /// ValidationReport / RiskScoreBreakdown / DecisionProposal are mirrored.
    pub async fn write_latest_with_session_mirror(
        &self,
        decision: &FinalDecision,
        session_store: &SessionJsonlStore,
    ) -> Result<()> {
        self.write_latest(decision).await?;
        let event = SessionEvent::ai_artifact(final_decision_artifact_event(decision)?);
        session_store.append(&event).with_context(|| {
            format!(
                "Failed to append final decision {} session artifact mirror for thread {} to {}",
                decision.decision_id,
                decision.thread_id,
                session_store.events_path().display()
            )
        })?;
        Ok(())
    }

    pub async fn load_latest(&self, thread_id: Uuid) -> Result<Option<FinalDecision>> {
        ai_final_decision::Entity::find()
            .filter(ai_final_decision::Column::ThreadId.eq(thread_id.to_string()))
            .filter(ai_final_decision::Column::IsLatest.eq(1))
            .order_by_desc(ai_final_decision::Column::CreatedAt)
            .one(&self.db)
            .await
            .with_context(|| format!("Failed to load latest final decision for {thread_id}"))?
            .map(final_decision_from_model)
            .transpose()
    }
}

pub fn final_decision_artifact_event(decision: &FinalDecision) -> Result<AiArtifactEvent> {
    Ok(AiArtifactEvent {
        event_id: Uuid::new_v4(),
        recorded_at: Utc::now(),
        thread_id: decision.thread_id,
        artifact_kind: "final_decision".to_string(),
        artifact_id: Some(decision.decision_id.to_string()),
        payload: serde_json::to_value(decision).with_context(|| {
            format!(
                "Failed to serialize final decision {} for session artifact mirror",
                decision.decision_id
            )
        })?,
    })
}

fn final_decision_to_active_model(
    decision: &FinalDecision,
) -> Result<ai_final_decision::ActiveModel> {
    Ok(ai_final_decision::ActiveModel {
        decision_id: Set(decision.decision_id.to_string()),
        thread_id: Set(decision.thread_id.to_string()),
        decision_proposal_id: Set(decision.decision_proposal_id.map(|id| id.to_string())),
        validation_report_id: Set(decision.validation_report_id.map(|id| id.to_string())),
        policy_version: Set(decision.policy_version.clone()),
        verdict: Set(decision.verdict.variant_name().to_string()),
        stale: Set(bool_to_row(decision.stale)),
        is_latest: Set(bool_to_row(decision.is_latest)),
        summary_json: Set(serialize_summary(
            &decision.summary,
            "final decision summary",
        )?),
        created_at: Set(decision.created_at.timestamp()),
        updated_at: Set(decision.updated_at.timestamp()),
    })
}

fn final_decision_from_model(row: ai_final_decision::Model) -> Result<FinalDecision> {
    let verdict = FinalDecisionVerdict::from_variant_name(&row.verdict).ok_or_else(|| {
        anyhow::anyhow!(
            "final decision {} carries unknown verdict tag '{}'",
            row.decision_id,
            row.verdict
        )
    })?;
    Ok(FinalDecision {
        decision_id: parse_uuid(&row.decision_id, "final decision_id")?,
        thread_id: parse_uuid(&row.thread_id, "final decision thread_id")?,
        decision_proposal_id: row
            .decision_proposal_id
            .as_deref()
            .map(|raw| parse_uuid(raw, "final decision decision_proposal_id"))
            .transpose()?,
        validation_report_id: row
            .validation_report_id
            .as_deref()
            .map(|raw| parse_uuid(raw, "final decision validation_report_id"))
            .transpose()?,
        policy_version: row.policy_version,
        verdict,
        stale: row.stale != 0,
        is_latest: row.is_latest != 0,
        summary: deserialize_summary(&row.summary_json, "final decision summary")?,
        created_at: timestamp_from_row(row.created_at, "final decision created_at")?,
        updated_at: timestamp_from_row(row.updated_at, "final decision updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::runtime::phase3::{
        ValidationReportSummary, ValidationStage, ValidationStageResult, ValidatorEngine,
    };

    fn sample_report(status: ValidationStatus) -> ValidationReport {
        let engine = ValidatorEngine::new("test:phase4");
        let outcome_stage = ValidationStageResult {
            stage: ValidationStage::Integration,
            outcome: match status {
                ValidationStatus::Passed => {
                    crate::internal::ai::runtime::phase3::ValidationOutcome::Passed
                }
                ValidationStatus::BlockingFailed => {
                    crate::internal::ai::runtime::phase3::ValidationOutcome::BlockingFailed
                }
                ValidationStatus::InfrastructureFailed => {
                    crate::internal::ai::runtime::phase3::ValidationOutcome::InfrastructureFailed
                }
            },
            evidence: vec![],
            summary: None,
        };
        let mut report = engine.build_report(Uuid::new_v4(), None, vec![outcome_stage]);
        // The engine always rolls up correctly, but pin the status here
        // for clarity in the tests.
        assert_eq!(report.summary.status, status);
        // Touch summary just to ensure compile reaches `summary`.
        let _ = &report.summary as *const ValidationReportSummary;
        report.policy_version = "test:phase4".to_string();
        report
    }

    /// `DecisionPolicy::default` must pin to the
    /// `DEFAULT_DECISION_POLICY_VERSION` constant and the well-known
    /// 30-score auto-accept threshold, so policy drift across versions
    /// is detected at compile time.
    #[test]
    fn decision_policy_default_pins_version_and_threshold() {
        let policy = DecisionPolicy::default();
        assert_eq!(policy.policy_version, DEFAULT_DECISION_POLICY_VERSION);
        assert_eq!(policy.policy_version, "decision:v1");
        assert_eq!(policy.auto_accept_max_score, 30);
    }

    /// `aggregate_risk_score` must produce the canonical score table:
    /// Passed=20, BlockingFailed=75, InfrastructureFailed=90.
    /// Pinning these values means a re-tune to the score weights breaks
    /// the test deliberately rather than silently shifting the
    /// auto-accept gate.
    #[test]
    fn aggregate_risk_score_pins_canonical_score_table() {
        let policy = DecisionPolicy::default();

        let passed = aggregate_risk_score(&sample_report(ValidationStatus::Passed), &policy);
        assert_eq!(passed.summary.score, 20);
        assert!(passed.summary.score <= policy.auto_accept_max_score);
        assert_eq!(passed.summary.validation_status, ValidationStatus::Passed);

        let blocking =
            aggregate_risk_score(&sample_report(ValidationStatus::BlockingFailed), &policy);
        assert_eq!(blocking.summary.score, 75);

        let infra = aggregate_risk_score(
            &sample_report(ValidationStatus::InfrastructureFailed),
            &policy,
        );
        assert_eq!(infra.summary.score, 90);
    }

    /// `build_decision_proposal` route table:
    /// - Passed + score ≤ threshold → AutoAccept (Accepted, no human review)
    /// - BlockingFailed → RequestChanges (Rejected, human review)
    /// - InfrastructureFailed → HumanReview (Abandon, human review)
    #[test]
    fn build_decision_proposal_routes_per_validation_status() {
        let policy = DecisionPolicy::default();

        let passed_report = sample_report(ValidationStatus::Passed);
        let passed_risk = aggregate_risk_score(&passed_report, &policy);
        let passed_proposal = build_decision_proposal(&passed_report, &passed_risk, &policy);
        assert_eq!(
            passed_proposal.summary.route,
            DecisionProposalRoute::AutoAccept
        );
        assert_eq!(
            passed_proposal.summary.proposed_verdict,
            FinalDecisionVerdict::Accepted
        );
        assert!(!passed_proposal.summary.requires_human_review);
        assert!(passed_proposal.is_auto_accept());

        let blocking_report = sample_report(ValidationStatus::BlockingFailed);
        let blocking_risk = aggregate_risk_score(&blocking_report, &policy);
        let blocking_proposal = build_decision_proposal(&blocking_report, &blocking_risk, &policy);
        assert_eq!(
            blocking_proposal.summary.route,
            DecisionProposalRoute::RequestChanges
        );
        assert_eq!(
            blocking_proposal.summary.proposed_verdict,
            FinalDecisionVerdict::Rejected
        );
        assert!(blocking_proposal.summary.requires_human_review);
        assert!(!blocking_proposal.is_auto_accept());

        let infra_report = sample_report(ValidationStatus::InfrastructureFailed);
        let infra_risk = aggregate_risk_score(&infra_report, &policy);
        let infra_proposal = build_decision_proposal(&infra_report, &infra_risk, &policy);
        assert_eq!(
            infra_proposal.summary.route,
            DecisionProposalRoute::HumanReview
        );
        assert_eq!(
            infra_proposal.summary.proposed_verdict,
            FinalDecisionVerdict::Abandon
        );
        assert!(infra_proposal.summary.requires_human_review);
        assert!(!infra_proposal.is_auto_accept());
    }

    /// `FinalDecision::finalize_auto_accept` finalises an AutoAccept proposal
    /// (carrying over verdict / route / risk_score / rationale) and refuses to
    /// finalise human-gated routes (returns None — those resolve via the
    /// CEX-S2-13 human-gated merge flow).
    #[test]
    fn finalize_auto_accept_only_finalises_auto_accept_route() {
        let policy = DecisionPolicy::default();
        let now = Utc::now();

        // Passed → AutoAccept → finalisable into an Accepted decision.
        let passed_report = sample_report(ValidationStatus::Passed);
        let passed_risk = aggregate_risk_score(&passed_report, &policy);
        let passed_proposal = build_decision_proposal(&passed_report, &passed_risk, &policy);
        assert!(passed_proposal.is_auto_accept());

        let decision = FinalDecision::finalize_auto_accept(&passed_proposal, now)
            .expect("AutoAccept proposal must finalise into a decision");
        assert_eq!(decision.verdict, FinalDecisionVerdict::Accepted);
        assert_eq!(decision.thread_id, passed_proposal.thread_id);
        assert_eq!(
            decision.decision_proposal_id,
            Some(passed_proposal.proposal_id)
        );
        assert_eq!(
            decision.validation_report_id,
            passed_proposal.validation_report_id
        );
        assert_eq!(decision.policy_version, passed_proposal.policy_version);
        assert_eq!(decision.summary.route, DecisionProposalRoute::AutoAccept);
        assert_eq!(
            decision.summary.risk_score,
            passed_proposal.summary.risk_score
        );
        assert!(decision.is_latest);
        assert!(!decision.stale);

        // Human-gated routes must NOT be auto-finalised.
        let blocking_report = sample_report(ValidationStatus::BlockingFailed);
        let blocking_risk = aggregate_risk_score(&blocking_report, &policy);
        let blocking_proposal = build_decision_proposal(&blocking_report, &blocking_risk, &policy);
        assert!(!blocking_proposal.is_auto_accept());
        assert!(
            FinalDecision::finalize_auto_accept(&blocking_proposal, now).is_none(),
            "a RequestChanges proposal must not auto-finalise"
        );

        let infra_report = sample_report(ValidationStatus::InfrastructureFailed);
        let infra_risk = aggregate_risk_score(&infra_report, &policy);
        let infra_proposal = build_decision_proposal(&infra_report, &infra_risk, &policy);
        assert!(
            FinalDecision::finalize_auto_accept(&infra_proposal, now).is_none(),
            "a HumanReview proposal must not auto-finalise"
        );

        // Freshness contract: a *stale* AutoAccept proposal must NOT be
        // auto-finalised — it must be replayed / recomputed first.
        let mut stale_proposal = passed_proposal.clone();
        stale_proposal.stale = true;
        assert!(stale_proposal.is_auto_accept());
        assert!(
            FinalDecision::finalize_auto_accept(&stale_proposal, now).is_none(),
            "a stale AutoAccept proposal must not auto-finalise"
        );
    }

    /// When validation passes but the risk score crosses the
    /// `auto_accept_max_score` threshold, the proposal must escalate to
    /// `HumanReview` (still proposes Accepted but requires review).
    #[test]
    fn build_decision_proposal_passed_above_threshold_routes_to_human_review() {
        let policy = DecisionPolicy {
            policy_version: "test:phase4".to_string(),
            auto_accept_max_score: 10, // force the Passed=20 score to exceed
        };
        let report = sample_report(ValidationStatus::Passed);
        let risk = aggregate_risk_score(&report, &policy);
        assert!(risk.summary.score > policy.auto_accept_max_score);

        let proposal = build_decision_proposal(&report, &risk, &policy);
        assert_eq!(proposal.summary.route, DecisionProposalRoute::HumanReview);
        assert_eq!(
            proposal.summary.proposed_verdict,
            FinalDecisionVerdict::Accepted
        );
        assert!(proposal.summary.requires_human_review);
    }

    /// Pin the **exact** inclusive auto-accept boundary
    /// (`risk.score <= policy.auto_accept_max_score`). The route-table
    /// test only exercises 20≤30 and 20>10; neither hits the precise
    /// edge. An off-by-one here (`<` instead of `<=`, or vice-versa) is
    /// a security-relevant defect: it would either auto-merge a
    /// sub-agent patch one risk point too risky, or force review on a
    /// patch that policy says is acceptable. This constructs a
    /// `Passed` report with a hand-built `RiskScoreBreakdown` at the
    /// threshold and one point above it.
    #[test]
    fn build_decision_proposal_auto_accept_boundary_is_inclusive() {
        let policy = DecisionPolicy {
            policy_version: "test:phase4".to_string(),
            auto_accept_max_score: 30,
        };
        let report = sample_report(ValidationStatus::Passed);

        let risk_at = |score: u8| RiskScoreBreakdown {
            breakdown_id: Uuid::new_v4(),
            thread_id: report.thread_id,
            validation_report_id: Some(report.report_id),
            policy_version: "test:phase4".to_string(),
            stale: false,
            is_latest: true,
            summary: RiskScoreSummary {
                score,
                reasons: vec![],
                validation_status: ValidationStatus::Passed,
            },
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        // score == threshold → AutoAccept (the boundary is inclusive).
        let at_threshold = build_decision_proposal(&report, &risk_at(30), &policy);
        assert_eq!(
            at_threshold.summary.route,
            DecisionProposalRoute::AutoAccept,
            "score exactly at auto_accept_max_score must auto-accept (`<=`)",
        );
        assert!(at_threshold.is_auto_accept());
        assert!(!at_threshold.summary.requires_human_review);

        // score == threshold + 1 → HumanReview (just over the edge).
        let over_threshold = build_decision_proposal(&report, &risk_at(31), &policy);
        assert_eq!(
            over_threshold.summary.route,
            DecisionProposalRoute::HumanReview,
            "score one point over the threshold must escalate to review",
        );
        assert!(!over_threshold.is_auto_accept());
        assert!(over_threshold.summary.requires_human_review);
        // A Passed-but-too-risky patch still proposes Accepted, just gated.
        assert_eq!(
            over_threshold.summary.proposed_verdict,
            FinalDecisionVerdict::Accepted
        );
    }

    /// `DecisionProposalRoute::variant_name` must match the
    /// `#[serde(rename_all = "snake_case")]` tag values for all four
    /// variants. Failure means audit logs (which use variant_name) and
    /// serialised payloads (which use the serde tag) drift apart.
    #[test]
    fn decision_proposal_route_variant_names_match_serde_tags() {
        for (route, expected) in [
            (DecisionProposalRoute::AutoAccept, "auto_accept"),
            (DecisionProposalRoute::HumanReview, "human_review"),
            (DecisionProposalRoute::RequestChanges, "request_changes"),
            (DecisionProposalRoute::Abandon, "abandon"),
        ] {
            assert_eq!(route.variant_name(), expected);
            let json = serde_json::to_string(&route).unwrap();
            // Serde writes it as a JSON string literal: "auto_accept".
            assert_eq!(json, format!("\"{expected}\""));
        }
    }

    /// `requires_human_review` must include every non-AutoAccept route
    /// (HumanReview, RequestChanges, Abandon). AutoAccept alone bypasses
    /// the human gate.
    #[test]
    fn decision_proposal_route_requires_human_review_excludes_auto_accept() {
        assert!(!DecisionProposalRoute::AutoAccept.requires_human_review());
        assert!(DecisionProposalRoute::HumanReview.requires_human_review());
        assert!(DecisionProposalRoute::RequestChanges.requires_human_review());
        assert!(DecisionProposalRoute::Abandon.requires_human_review());
    }
}
