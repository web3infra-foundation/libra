use serde::{Deserialize, Serialize};

use super::types::{ChangeType, CheckKind, Objective, RiskLevel, TouchHints};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IntentDraft {
    pub intent: DraftIntent,
    pub acceptance: DraftAcceptance,
    pub risk: DraftRisk,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DraftIntent {
    pub summary: String,
    #[serde(rename = "problemStatement")]
    pub problem_statement: String,
    #[serde(rename = "changeType")]
    pub change_type: ChangeType,
    pub objectives: Vec<Objective>,
    #[serde(rename = "inScope")]
    pub in_scope: Vec<String>,
    #[serde(rename = "outOfScope", default)]
    pub out_of_scope: Vec<String>,
    #[serde(
        rename = "touchHints",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub touch_hints: Option<TouchHints>,
}

impl DraftIntent {
    pub fn has_implementation_objectives(&self) -> bool {
        self.objectives
            .iter()
            .any(|objective| objective.kind == super::types::ObjectiveKind::Implementation)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DraftAcceptance {
    #[serde(rename = "successCriteria")]
    pub success_criteria: Vec<String>,
    #[serde(rename = "fastChecks", default)]
    pub fast_checks: Vec<DraftCheck>,
    #[serde(rename = "integrationChecks", default)]
    pub integration_checks: Vec<DraftCheck>,
    #[serde(rename = "securityChecks", default)]
    pub security_checks: Vec<DraftCheck>,
    #[serde(rename = "releaseChecks", default)]
    pub release_checks: Vec<DraftCheck>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DraftCheck {
    pub id: String,
    pub kind: CheckKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(
        rename = "timeoutSeconds",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_seconds: Option<u64>,
    #[serde(
        rename = "expectedExitCode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub expected_exit_code: Option<i32>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(rename = "artifactsProduced", default)]
    pub artifacts_produced: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DraftRisk {
    pub rationale: String,
    #[serde(default)]
    pub factors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<RiskLevel>,
}
