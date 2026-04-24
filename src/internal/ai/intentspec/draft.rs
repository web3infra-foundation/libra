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
            #[serde(default)]
            description: Option<String>,
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
        let description = normalize_optional_string(input.description);
        let mut command = normalize_optional_string(input.command);
        if command.is_none() {
            command = description
                .as_deref()
                .and_then(infer_command_from_check_description);
        }

        let kind = match input.kind {
            Some(kind) => kind,
            None if command
                .as_deref()
                .is_some_and(|command| !command.is_empty()) =>
            {
                CheckKind::Command
            }
            None => CheckKind::Policy,
        };
        let id = input
            .id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                derive_check_id(&kind, command.as_deref().or(description.as_deref()))
            });

        Ok(Self {
            id,
            kind,
            command,
            timeout_seconds: input.timeout_seconds,
            expected_exit_code: input.expected_exit_code,
            required: input.required,
            artifacts_produced: input.artifacts_produced,
        })
    }
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn infer_command_from_check_description(description: &str) -> Option<String> {
    let trimmed = description.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        return None;
    }

    if let Some(command) = quoted_prefix(trimmed) {
        return Some(command);
    }

    let lower = trimmed.to_ascii_lowercase();
    let command_prefixes = [
        "cargo ", "libra ", "git ", "npm ", "pnpm ", "yarn ", "make", "cmake ", "pytest",
        "python ", "python3 ", "go ", "rustc ",
    ];
    if !command_prefixes.iter().any(|prefix| {
        lower == *prefix
            || lower
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || !rest.starts_with(char::is_whitespace))
    }) {
        return None;
    }

    let command = strip_description_command_suffix(trimmed).trim();
    (!command.is_empty()).then(|| command.to_string())
}

fn quoted_prefix(description: &str) -> Option<String> {
    let mut chars = description.chars();
    let quote = chars.next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }

    let close = description[quote.len_utf8()..].find(quote)?;
    let command = &description[quote.len_utf8()..quote.len_utf8() + close];
    let command = command.trim();
    (!command.is_empty()).then(|| command.to_string())
}

fn strip_description_command_suffix(description: &str) -> &str {
    let lower = description.to_ascii_lowercase();
    [
        " succeeds",
        " passes",
        " should pass",
        " exits with",
        " exits ",
        " returns ",
        " output contains",
        " prints ",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .min()
    .map_or(description, |index| &description[..index])
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
    fn draft_check_missing_kind_without_command_defaults_to_policy() {
        let check: DraftCheck = serde_json::from_value(serde_json::json!({
            "description": "No hardcoded secrets in source files",
            "required": true
        }))
        .unwrap();

        assert_eq!(check.id, "no-hardcoded-secrets-in-source-files");
        assert_eq!(check.kind, CheckKind::Policy);
        assert_eq!(check.command, None);
    }

    #[test]
    fn draft_check_infers_command_from_description() {
        let check: DraftCheck = serde_json::from_value(serde_json::json!({
            "description": "cargo build --manifest-path /home/eli/linked/Cargo.toml succeeds",
            "required": true
        }))
        .unwrap();

        assert_eq!(
            check.command.as_deref(),
            Some("cargo build --manifest-path /home/eli/linked/Cargo.toml")
        );
        assert_eq!(check.kind, CheckKind::Command);
    }

    #[test]
    fn draft_check_accepts_kind_aliases() {
        let build: DraftCheck = serde_json::from_value(serde_json::json!({
            "kind": "build",
            "description": "cargo build succeeds"
        }))
        .unwrap();
        let test: DraftCheck = serde_json::from_value(serde_json::json!({
            "kind": "test",
            "description": "cargo test succeeds"
        }))
        .unwrap();

        assert_eq!(build.kind, CheckKind::Command);
        assert_eq!(build.command.as_deref(), Some("cargo build"));
        assert_eq!(test.kind, CheckKind::TestSuite);
        assert_eq!(test.command.as_deref(), Some("cargo test"));
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
