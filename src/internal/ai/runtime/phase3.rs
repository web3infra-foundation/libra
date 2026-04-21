//! Phase 3 validation pipeline and derived-record persistence.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::internal::{
    ai::runtime::{contracts::EvidenceKind, derived_records::ensure_runtime_thread},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Passed,
    BlockingFailed,
    InfrastructureFailed,
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
