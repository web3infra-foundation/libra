//! Phase 3 validation pipeline and derived-record persistence.
//!
//! 阶段 3 验证管道和派生记录持久化。

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::internal::{
    ai::{
        runtime::{contracts::EvidenceKind, derived_records::ensure_runtime_thread},
        session::jsonl::{AiArtifactEvent, SessionEvent, SessionJsonlStore},
    },
    model::ai_validation_report,
};

pub const DEFAULT_VALIDATION_POLICY_VERSION: &str = "validation:v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactLedger {
    pub thread_id: Uuid,
    #[serde(default)]
    pub tasks: Vec<TaskArtifactRefs>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskArtifactRefs {
    pub task_id: Uuid,
    #[serde(default)]
    pub patchset_ids: Vec<Uuid>,
    #[serde(default)]
    pub evidence: Vec<EvidenceKind>,
    #[serde(default)]
    pub context_frame_ids: Vec<Uuid>,
    #[serde(default)]
    pub usage_ids: Vec<Uuid>,
}

impl ArtifactLedger {
    pub fn new(thread_id: Uuid) -> Self {
        Self {
            thread_id,
            tasks: Vec::new(),
        }
    }

    pub fn push_task(&mut self, task: TaskArtifactRefs) {
        self.tasks.push(task);
    }

    pub fn has_patchset(&self, patchset_id: Uuid) -> bool {
        self.tasks
            .iter()
            .any(|task| task.patchset_ids.contains(&patchset_id))
    }

    pub fn release_candidate_patchset_id(&self) -> Option<Uuid> {
        self.tasks
            .iter()
            .rev()
            .find_map(|task| task.patchset_ids.last().copied())
    }
}

impl TaskArtifactRefs {
    pub fn new(task_id: Uuid) -> Self {
        Self {
            task_id,
            patchset_ids: Vec::new(),
            evidence: Vec::new(),
            context_frame_ids: Vec::new(),
            usage_ids: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    Integration,
    Security,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationOutcome {
    Passed,
    BlockingFailed,
    InfrastructureFailed,
}

impl ValidationOutcome {
    /// `true` only for `Passed`. Used to roll per-stage outcomes up into
    /// the report-level [`ValidationStatus`] in
    /// [`validation_status`](self::validation_status); also useful for
    /// quick "did this stage clear?" checks at observer call sites.
    pub fn is_passing(self) -> bool {
        matches!(self, ValidationOutcome::Passed)
    }

    /// `true` for the non-recoverable infrastructure-failed category.
    /// Distinguished from `BlockingFailed` because infrastructure
    /// failures cannot be auto-retried by Phase 3 routing — they
    /// require a Phase 5 diagnostic loop instead.
    pub fn is_infrastructure_failure(self) -> bool {
        matches!(self, ValidationOutcome::InfrastructureFailed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Passed,
    BlockingFailed,
    InfrastructureFailed,
}

impl ValidationStatus {
    /// `true` only for `Passed`. The Phase 4 decision pipeline
    /// (see `phase4.rs:101..`) branches on this when computing the
    /// auto-accept gate.
    pub fn is_passing(self) -> bool {
        matches!(self, ValidationStatus::Passed)
    }

    /// `true` for the non-recoverable infrastructure-failed category.
    /// Phase 4 escalates these to a Phase 5 diagnostic loop instead of
    /// the standard retry path.
    pub fn is_infrastructure_failure(self) -> bool {
        matches!(self, ValidationStatus::InfrastructureFailed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationStageResult {
    pub stage: ValidationStage,
    pub outcome: ValidationOutcome,
    #[serde(default)]
    pub evidence: Vec<EvidenceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReportSummary {
    pub status: ValidationStatus,
    #[serde(default)]
    pub stages: Vec<ValidationStageResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub report_id: Uuid,
    pub thread_id: Uuid,
    pub run_id: Option<Uuid>,
    pub policy_version: String,
    pub stale: bool,
    pub is_latest: bool,
    pub summary: ValidationReportSummary,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ValidationReport {
    /// Convenience: `true` when the report's roll-up status is
    /// [`ValidationStatus::Passed`]. Useful for callers that only need
    /// the report-level pass/fail signal and don't want to drill into
    /// `report.summary.status` themselves.
    pub fn is_passing(&self) -> bool {
        self.summary.status.is_passing()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorEngine {
    policy_version: String,
}

impl ValidatorEngine {
    pub fn new(policy_version: impl Into<String>) -> Self {
        Self {
            policy_version: policy_version.into(),
        }
    }

    pub fn default_policy() -> Self {
        Self::new(DEFAULT_VALIDATION_POLICY_VERSION)
    }

    pub fn build_report(
        &self,
        thread_id: Uuid,
        run_id: Option<Uuid>,
        stages: Vec<ValidationStageResult>,
    ) -> ValidationReport {
        let now = Utc::now();
        ValidationReport {
            report_id: Uuid::new_v4(),
            thread_id,
            run_id,
            policy_version: self.policy_version.clone(),
            stale: false,
            is_latest: true,
            summary: ValidationReportSummary {
                status: validation_status(&stages),
                stages,
            },
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Clone)]
pub struct ValidationReportStore {
    db: DatabaseConnection,
}

impl ValidationReportStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn write_latest(&self, report: &ValidationReport) -> Result<()> {
        let txn = self
            .db
            .begin()
            .await
            .context("Failed to start validation report transaction")?;

        ensure_runtime_thread(&txn, report.thread_id).await?;

        ai_validation_report::Entity::update_many()
            .col_expr(ai_validation_report::Column::IsLatest, Expr::value(0))
            .filter(ai_validation_report::Column::ThreadId.eq(report.thread_id.to_string()))
            .exec(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to clear previous latest validation report for thread {}",
                    report.thread_id
                )
            })?;

        report_to_active_model(report)?
            .insert(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to insert validation report {} for thread {}",
                    report.report_id, report.thread_id
                )
            })?;

        txn.commit()
            .await
            .context("Failed to commit validation report transaction")?;
        Ok(())
    }

    pub async fn write_latest_with_session_mirror(
        &self,
        report: &ValidationReport,
        session_store: &SessionJsonlStore,
    ) -> Result<()> {
        self.write_latest(report).await?;
        append_validation_report_session_mirror(session_store, report)?;
        Ok(())
    }

    pub async fn load_latest(&self, thread_id: Uuid) -> Result<Option<ValidationReport>> {
        ai_validation_report::Entity::find()
            .filter(ai_validation_report::Column::ThreadId.eq(thread_id.to_string()))
            .filter(ai_validation_report::Column::IsLatest.eq(1))
            .order_by_desc(ai_validation_report::Column::CreatedAt)
            .one(&self.db)
            .await
            .with_context(|| format!("Failed to load latest validation report for {thread_id}"))?
            .map(report_from_model)
            .transpose()
    }
}

pub fn append_validation_report_session_mirror(
    session_store: &SessionJsonlStore,
    report: &ValidationReport,
) -> Result<()> {
    let event = SessionEvent::ai_artifact(validation_report_artifact_event(report)?);
    session_store.append(&event).with_context(|| {
        format!(
            "Failed to append validation report {} session artifact mirror for thread {} to {}",
            report.report_id,
            report.thread_id,
            session_store.events_path().display()
        )
    })
}

pub fn validation_report_artifact_event(report: &ValidationReport) -> Result<AiArtifactEvent> {
    Ok(AiArtifactEvent {
        event_id: Uuid::new_v4(),
        recorded_at: Utc::now(),
        thread_id: report.thread_id,
        artifact_kind: "validation_report".to_string(),
        artifact_id: Some(report.report_id.to_string()),
        payload: serde_json::to_value(report).with_context(|| {
            format!(
                "Failed to serialize validation report {} for session artifact mirror",
                report.report_id
            )
        })?,
    })
}

fn validation_status(stages: &[ValidationStageResult]) -> ValidationStatus {
    if stages
        .iter()
        .any(|stage| stage.outcome == ValidationOutcome::InfrastructureFailed)
    {
        ValidationStatus::InfrastructureFailed
    } else if stages
        .iter()
        .any(|stage| stage.outcome == ValidationOutcome::BlockingFailed)
    {
        ValidationStatus::BlockingFailed
    } else {
        ValidationStatus::Passed
    }
}

fn report_to_active_model(report: &ValidationReport) -> Result<ai_validation_report::ActiveModel> {
    Ok(ai_validation_report::ActiveModel {
        report_id: Set(report.report_id.to_string()),
        thread_id: Set(report.thread_id.to_string()),
        run_id: Set(report.run_id.map(|id| id.to_string())),
        policy_version: Set(report.policy_version.clone()),
        stale: Set(bool_to_row(report.stale)),
        is_latest: Set(bool_to_row(report.is_latest)),
        summary_json: Set(serialize_summary(
            &report.summary,
            "validation report summary",
        )?),
        created_at: Set(report.created_at.timestamp()),
        updated_at: Set(report.updated_at.timestamp()),
    })
}

fn report_from_model(row: ai_validation_report::Model) -> Result<ValidationReport> {
    Ok(ValidationReport {
        report_id: parse_uuid(&row.report_id, "validation report_id")?,
        thread_id: parse_uuid(&row.thread_id, "validation thread_id")?,
        run_id: row
            .run_id
            .as_deref()
            .map(|raw| parse_uuid(raw, "validation run_id"))
            .transpose()?,
        policy_version: row.policy_version,
        stale: row.stale != 0,
        is_latest: row.is_latest != 0,
        summary: deserialize_summary(&row.summary_json, "validation report summary")?,
        created_at: timestamp_from_row(row.created_at, "validation created_at")?,
        updated_at: timestamp_from_row(row.updated_at, "validation updated_at")?,
    })
}

pub(crate) fn serialize_summary<T: Serialize>(value: &T, label: &str) -> Result<String> {
    serde_json::to_string(value).with_context(|| format!("Failed to serialize {label}"))
}

pub(crate) fn deserialize_summary<T: DeserializeOwned>(raw: &str, label: &str) -> Result<T> {
    serde_json::from_str(raw).with_context(|| format!("Failed to parse {label}"))
}

pub(crate) fn parse_uuid(raw: &str, label: &str) -> Result<Uuid> {
    Uuid::parse_str(raw).with_context(|| format!("Invalid {label} UUID: {raw}"))
}

pub(crate) fn timestamp_from_row(raw: i64, label: &str) -> Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(raw, 0)
        .with_context(|| format!("Invalid {label} timestamp: {raw}"))
}

pub(crate) fn bool_to_row(value: bool) -> i64 {
    i64::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ValidationOutcome::is_passing` must return `true` only for
    /// `Passed`. The two failure variants are *not* passing and Phase 3
    /// rollup logic relies on that.
    #[test]
    fn validation_outcome_is_passing_only_for_passed() {
        assert!(ValidationOutcome::Passed.is_passing());
        assert!(!ValidationOutcome::BlockingFailed.is_passing());
        assert!(!ValidationOutcome::InfrastructureFailed.is_passing());
    }

    /// `is_infrastructure_failure` must distinguish the non-recoverable
    /// infrastructure-failed category from `BlockingFailed` so Phase 3
    /// routing escalates them differently.
    #[test]
    fn validation_outcome_is_infrastructure_failure_only_for_infra() {
        assert!(!ValidationOutcome::Passed.is_infrastructure_failure());
        assert!(!ValidationOutcome::BlockingFailed.is_infrastructure_failure());
        assert!(ValidationOutcome::InfrastructureFailed.is_infrastructure_failure());
    }

    /// `ValidationStatus::is_passing` must return `true` only for
    /// `Passed` — mirrors the per-stage `ValidationOutcome::is_passing`
    /// semantics at the report-level rollup.
    #[test]
    fn validation_status_is_passing_only_for_passed() {
        assert!(ValidationStatus::Passed.is_passing());
        assert!(!ValidationStatus::BlockingFailed.is_passing());
        assert!(!ValidationStatus::InfrastructureFailed.is_passing());
    }

    /// Mirror of the per-stage predicate at the report-level enum.
    #[test]
    fn validation_status_is_infrastructure_failure_only_for_infra() {
        assert!(!ValidationStatus::Passed.is_infrastructure_failure());
        assert!(!ValidationStatus::BlockingFailed.is_infrastructure_failure());
        assert!(ValidationStatus::InfrastructureFailed.is_infrastructure_failure());
    }

    /// `validation_status` rollup priority: Infrastructure > Blocking >
    /// Passed. A single Infrastructure failure dominates everything; a
    /// single BlockingFailed (without any Infrastructure) dominates
    /// passed stages.
    #[test]
    fn validation_status_rollup_honours_failure_priority() {
        // All-passed → Passed.
        let stages = vec![ValidationStageResult {
            stage: ValidationStage::Integration,
            outcome: ValidationOutcome::Passed,
            evidence: vec![],
            summary: None,
        }];
        assert_eq!(validation_status(&stages), ValidationStatus::Passed);

        // BlockingFailed dominates Passed.
        let stages = vec![
            ValidationStageResult {
                stage: ValidationStage::Integration,
                outcome: ValidationOutcome::Passed,
                evidence: vec![],
                summary: None,
            },
            ValidationStageResult {
                stage: ValidationStage::Security,
                outcome: ValidationOutcome::BlockingFailed,
                evidence: vec![],
                summary: None,
            },
        ];
        assert_eq!(validation_status(&stages), ValidationStatus::BlockingFailed);

        // InfrastructureFailed dominates BlockingFailed.
        let stages = vec![
            ValidationStageResult {
                stage: ValidationStage::Integration,
                outcome: ValidationOutcome::BlockingFailed,
                evidence: vec![],
                summary: None,
            },
            ValidationStageResult {
                stage: ValidationStage::Security,
                outcome: ValidationOutcome::InfrastructureFailed,
                evidence: vec![],
                summary: None,
            },
        ];
        assert_eq!(
            validation_status(&stages),
            ValidationStatus::InfrastructureFailed
        );

        // Empty stages → Passed (vacuously, no failure observed).
        assert_eq!(validation_status(&[]), ValidationStatus::Passed);
    }

    /// `ValidatorEngine::build_report` + `ValidationReport::is_passing`:
    /// engine rolls up per-stage outcomes; report-level convenience
    /// helper matches the rolled-up status.
    #[test]
    fn validation_report_is_passing_matches_summary_status() {
        let engine = ValidatorEngine::new("test-policy");
        let thread_id = Uuid::new_v4();

        let passing = engine.build_report(
            thread_id,
            None,
            vec![ValidationStageResult {
                stage: ValidationStage::Integration,
                outcome: ValidationOutcome::Passed,
                evidence: vec![],
                summary: None,
            }],
        );
        assert!(passing.is_passing());
        assert_eq!(passing.summary.status, ValidationStatus::Passed);
        assert_eq!(passing.policy_version, "test-policy");

        let failing = engine.build_report(
            thread_id,
            None,
            vec![ValidationStageResult {
                stage: ValidationStage::Security,
                outcome: ValidationOutcome::BlockingFailed,
                evidence: vec![],
                summary: None,
            }],
        );
        assert!(!failing.is_passing());
        assert_eq!(failing.summary.status, ValidationStatus::BlockingFailed);
    }

    /// ArtifactLedger exposes the latest task patchset as the release
    /// candidate and can prove that a selected patchset belongs to the
    /// ledger before Phase 3 marks validation as passing.
    #[test]
    fn artifact_ledger_tracks_release_candidate_patchset() {
        let thread_id = Uuid::new_v4();
        let first_patchset_id = Uuid::new_v4();
        let release_patchset_id = Uuid::new_v4();
        let mut ledger = ArtifactLedger::new(thread_id);

        let mut first = TaskArtifactRefs::new(Uuid::new_v4());
        first.patchset_ids.push(first_patchset_id);
        ledger.push_task(first);

        let mut second = TaskArtifactRefs::new(Uuid::new_v4());
        second.patchset_ids.push(release_patchset_id);
        ledger.push_task(second);

        assert!(ledger.has_patchset(first_patchset_id));
        assert!(ledger.has_patchset(release_patchset_id));
        assert!(!ledger.has_patchset(Uuid::new_v4()));
        assert_eq!(
            ledger.release_candidate_patchset_id(),
            Some(release_patchset_id)
        );
    }

    /// `ValidatorEngine::default_policy` must use the
    /// `DEFAULT_VALIDATION_POLICY_VERSION` constant so policy-version
    /// drift between code and reports is detected at compile time.
    #[test]
    fn validator_engine_default_policy_uses_pinned_constant() {
        let engine = ValidatorEngine::default_policy();
        let report = engine.build_report(Uuid::new_v4(), None, vec![]);
        assert_eq!(report.policy_version, DEFAULT_VALIDATION_POLICY_VERSION);
        assert_eq!(report.policy_version, "validation:v1");
    }
}
