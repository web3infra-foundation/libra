//! Static skill scanner for risky workflow definitions.

use serde::{Deserialize, Serialize};

use super::parser::SkillDefinition;

/// Severity for a skill scanner warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScanSeverity {
    Warning,
    Deny,
}

/// Scanner warning surfaced when loading a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillScanWarning {
    pub severity: SkillScanSeverity,
    pub rule: String,
    pub message: String,
}

impl SkillScanWarning {
    fn warning(rule: &str, message: &str) -> Self {
        Self {
            severity: SkillScanSeverity::Warning,
            rule: rule.to_string(),
            message: message.to_string(),
        }
    }

    fn deny(rule: &str, message: &str) -> Self {
        Self {
            severity: SkillScanSeverity::Deny,
            rule: rule.to_string(),
            message: message.to_string(),
        }
    }
}

/// Scan a skill for risky tool policy and obvious unsafe snippets.
pub fn scan_skill(skill: &SkillDefinition) -> Vec<SkillScanWarning> {
    let mut warnings = Vec::new();

    if skill.allowed_tools.is_empty() {
        warnings.push(SkillScanWarning::warning(
            "missing_allowed_tools",
            "skill does not declare allowed-tools; mutating tools will not be inherited automatically",
        ));
    }

    for tool in &skill.allowed_tools {
        if matches!(tool.as_str(), "shell" | "apply_patch" | "run_libra_vcs") {
            warnings.push(SkillScanWarning::warning(
                "broad_or_mutating_tool",
                "skill declares a mutating or broad tool; review before enabling in automation",
            ));
        }
    }

    let lower = skill.template.to_ascii_lowercase();
    for marker in ["rm -rf", "curl ", "wget ", "~/.ssh", "credential", "secret"] {
        if lower.contains(marker) {
            warnings.push(SkillScanWarning::warning(
                "suspicious_template_marker",
                "skill template contains shell/network/credential markers that require review",
            ));
            break;
        }
    }

    for marker in ["aws_secret_access_key", "github_token", "openai_api_key"] {
        if lower.contains(marker) {
            warnings.push(SkillScanWarning::deny(
                "credential_exfiltration_marker",
                "skill template references a known credential marker",
            ));
            break;
        }
    }

    warnings
}
