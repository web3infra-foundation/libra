use serde::{Deserialize, Deserializer, Serialize};

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

#[derive(Clone, Debug, Serialize, PartialEq)]
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

impl<'de> Deserialize<'de> for DraftCheck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct DraftCheckInput {
            #[serde(default)]
            id: Option<String>,
            #[serde(default)]
            kind: Option<CheckKind>,
            #[serde(default)]
            command: Option<String>,
            #[serde(rename = "timeoutSeconds", default)]
            timeout_seconds: Option<u64>,
            #[serde(rename = "expectedExitCode", default)]
            expected_exit_code: Option<i32>,
            #[serde(default = "default_true")]
            required: bool,
            #[serde(rename = "artifactsProduced", default)]
            artifacts_produced: Vec<String>,
        }

        let input = DraftCheckInput::deserialize(deserializer)?;
        let kind = match input.kind {
            Some(kind) => kind,
            None if input
                .command
                .as_deref()
                .map(str::trim)
                .is_some_and(|command| !command.is_empty()) =>
            {
                CheckKind::Command
            }
            None => {
                return Err(<D::Error as serde::de::Error>::custom(
                    "check.kind is required when command is absent",
                ));
            }
        };
        let id = input
            .id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| derive_check_id(&kind, input.command.as_deref()));

        Ok(Self {
            id,
            kind,
            command: input.command,
            timeout_seconds: input.timeout_seconds,
            expected_exit_code: input.expected_exit_code,
            required: input.required,
            artifacts_produced: input.artifacts_produced,
        })
    }
}

fn derive_check_id(kind: &CheckKind, command: Option<&str>) -> String {
    let source = command
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .unwrap_or(match kind {
            CheckKind::Command => "command-check",
            CheckKind::TestSuite => "test-suite-check",
            CheckKind::Policy => "policy-check",
        });
    let slug = slugify_check_id(source);
    if slug.is_empty() {
        "check".to_string()
    } else {
        slug
    }
}

fn slugify_check_id(value: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }

        if out.len() >= 64 {
            break;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_check_missing_id_is_derived_from_command() {
        let check: DraftCheck = serde_json::from_value(serde_json::json!({
            "kind": "command",
            "command": "cargo check --all",
            "artifactsProduced": []
        }))
        .unwrap();

        assert_eq!(check.id, "cargo-check-all");
        assert_eq!(check.kind, CheckKind::Command);
        assert_eq!(check.command.as_deref(), Some("cargo check --all"));
        assert!(check.required);
    }

    #[test]
    fn draft_check_missing_kind_is_derived_from_command() {
        let check: DraftCheck = serde_json::from_value(serde_json::json!({
            "command": "cargo check",
            "timeoutSeconds": 120,
            "expectedExitCode": 0,
            "required": true,
            "artifactsProduced": []
        }))
        .unwrap();

        assert_eq!(check.id, "cargo-check");
        assert_eq!(check.kind, CheckKind::Command);
        assert_eq!(check.command.as_deref(), Some("cargo check"));
        assert_eq!(check.timeout_seconds, Some(120));
        assert_eq!(check.expected_exit_code, Some(0));
        assert!(check.required);
    }

    #[test]
    fn draft_check_missing_kind_without_command_is_rejected() {
        let err = serde_json::from_value::<DraftCheck>(serde_json::json!({
            "required": true
        }))
        .expect_err("checks without kind or command are ambiguous");

        assert!(err.to_string().contains("command is absent"));
    }

    #[test]
    fn draft_check_missing_id_falls_back_to_kind() {
        let check: DraftCheck = serde_json::from_value(serde_json::json!({
            "kind": "policy"
        }))
        .unwrap();

        assert_eq!(check.id, "policy-check");
        assert_eq!(check.kind, CheckKind::Policy);
    }
}
