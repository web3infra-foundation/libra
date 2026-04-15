//! Phase D validation and decision derived-record tests.

use libra::internal::{
    ai::runtime::{
        DecisionPolicy, DecisionProposalRoute, DecisionProposalStore, ValidationOutcome,
        ValidationReportStore, ValidationStage, ValidationStageResult, ValidatorEngine,
        aggregate_risk_score, build_decision_proposal,
        contracts::{EvidenceKind, FinalDecisionVerdict},
        phase3::ValidationStatus,
    },
    model::{ai_thread, ai_validation_report},
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, Database, EntityTrait,
    QueryFilter, Statement,
};
use uuid::Uuid;

const BOOTSTRAP_SQL: &str = include_str!("../sql/sqlite_20260309_init.sql");

async fn setup_db() -> sea_orm::DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    db.execute(Statement::from_string(
        db.get_database_backend(),
        BOOTSTRAP_SQL,
    ))
    .await
    .unwrap();
    db
}

#[tokio::test]
async fn validation_reports_and_decision_proposals_are_latest_derived_records() {
    let db = setup_db().await;
    let thread_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    ai_thread::ActiveModel {
        thread_id: Set(thread_id.to_string()),
        title: Set(Some("validation decision flow".to_string())),
        owner_kind: Set("human".to_string()),
        owner_id: Set("tester".to_string()),
        owner_display_name: Set(None),
        current_intent_id: Set(None),
        latest_intent_id: Set(None),
        metadata_json: Set(None),
        archived: Set(false),
        version: Set(1),
        created_at: Set(1_700_000_000),
        updated_at: Set(1_700_000_000),
    }
    .insert(&db)
    .await
    .unwrap();
    let validator = ValidatorEngine::default_policy();
    let validation_store = ValidationReportStore::new(db.clone());
    let decision_store = DecisionProposalStore::new(db.clone());
    let policy = DecisionPolicy::default();

    let passed = validator.build_report(
        thread_id,
        Some(run_id),
        vec![
            ValidationStageResult {
                stage: ValidationStage::Integration,
                outcome: ValidationOutcome::Passed,
                evidence: vec![EvidenceKind::Test],
                summary: Some("cargo test passed".to_string()),
            },
            ValidationStageResult {
                stage: ValidationStage::Security,
                outcome: ValidationOutcome::Passed,
                evidence: vec![EvidenceKind::Security],
                summary: Some("no security blockers".to_string()),
            },
            ValidationStageResult {
                stage: ValidationStage::Release,
                outcome: ValidationOutcome::Passed,
                evidence: vec![EvidenceKind::Build],
                summary: Some("release checks passed".to_string()),
            },
        ],
    );
    validation_store.write_latest(&passed).await.unwrap();

    let passed_risk = aggregate_risk_score(&passed, &policy);
    let passed_proposal = build_decision_proposal(&passed, &passed_risk, &policy);
    decision_store
        .write_latest(&passed_risk, &passed_proposal)
        .await
        .unwrap();

    let loaded_proposal = decision_store
        .load_latest_proposal(thread_id)
        .await
        .unwrap()
        .expect("decision proposal");
    assert_eq!(
        loaded_proposal.summary.route,
        DecisionProposalRoute::AutoAccept
    );
    assert_eq!(
        loaded_proposal.summary.proposed_verdict,
        FinalDecisionVerdict::Accepted
    );
    assert!(!loaded_proposal.summary.requires_human_review);

    let blocking = validator.build_report(
        thread_id,
        Some(run_id),
        vec![ValidationStageResult {
            stage: ValidationStage::Integration,
            outcome: ValidationOutcome::BlockingFailed,
            evidence: vec![EvidenceKind::ValidationBlockingFailed],
            summary: Some("required test failed".to_string()),
        }],
    );
    validation_store.write_latest(&blocking).await.unwrap();
    let blocking_risk = aggregate_risk_score(&blocking, &policy);
    let blocking_proposal = build_decision_proposal(&blocking, &blocking_risk, &policy);
    decision_store
        .write_latest(&blocking_risk, &blocking_proposal)
        .await
        .unwrap();

    let latest_report = validation_store
        .load_latest(thread_id)
        .await
        .unwrap()
        .expect("latest validation report");
    assert_eq!(latest_report.report_id, blocking.report_id);
    assert_eq!(
        latest_report.summary.status,
        ValidationStatus::BlockingFailed
    );

    let latest_proposal = decision_store
        .load_latest_proposal(thread_id)
        .await
        .unwrap()
        .expect("latest decision proposal");
    assert_eq!(
        latest_proposal.summary.route,
        DecisionProposalRoute::RequestChanges
    );
    assert_eq!(
        latest_proposal.summary.proposed_verdict,
        FinalDecisionVerdict::Rejected
    );
    assert!(latest_proposal.summary.requires_human_review);

    let latest_rows = ai_validation_report::Entity::find()
        .filter(ai_validation_report::Column::ThreadId.eq(thread_id.to_string()))
        .filter(ai_validation_report::Column::IsLatest.eq(1))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(latest_rows.len(), 1);
}
