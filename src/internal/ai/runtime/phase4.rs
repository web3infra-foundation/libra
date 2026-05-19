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
    ai::runtime::{
        contracts::FinalDecisionVerdict,
        derived_records::ensure_runtime_thread,
        phase3::{
            ValidationReport, ValidationStatus, bool_to_row, deserialize_summary, parse_uuid,
            serialize_summary, timestamp_from_row,
        },
    },
    model::{ai_decision_proposal, ai_risk_score_breakdown},
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
