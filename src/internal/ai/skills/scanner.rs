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

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_with(allowed_tools: Vec<&str>, template: &str) -> SkillDefinition {
        SkillDefinition {
            name: "test".to_string(),
            description: "test skill".to_string(),
            version: None,
            allowed_tools: allowed_tools.into_iter().map(str::to_string).collect(),
            template: template.to_string(),
            checksum: "0".to_string(),
            source_path: None,
            warnings: Vec::new(),
        }
    }

    /// `scan_skill` clean baseline: a skill with at least one safe tool
    /// and an innocuous template produces no warnings. Pin the
    /// "no false positives on safe shapes" rule.
    #[test]
    fn scan_skill_clean_shape_produces_no_warnings() {
        let skill = skill_with(vec!["read_file"], "Read the file and report.");
        assert!(scan_skill(&skill).is_empty());
    }

    /// Missing `allowed-tools` declaration triggers a Warning with
    /// rule `missing_allowed_tools`. Pin the rule name so audit
    /// consumers can grep on it.
    #[test]
    fn scan_skill_missing_allowed_tools_triggers_warning() {
        let skill = skill_with(vec![], "safe template");
        let warnings = scan_skill(&skill);
        assert!(warnings.iter().any(|w| w.rule == "missing_allowed_tools"));
        let issue = warnings
            .iter()
            .find(|w| w.rule == "missing_allowed_tools")
            .unwrap();
        assert_eq!(issue.severity, SkillScanSeverity::Warning);
        assert!(
            issue.message.contains("allowed-tools"),
            "message must mention the missing field; got {}",
            issue.message,
        );
    }

    /// Broad/mutating tools (`shell`, `apply_patch`, `run_libra_vcs`)
    /// each fire a `broad_or_mutating_tool` warning at Warning
    /// severity. Pin the canonical 3-tool blocklist.
    #[test]
    fn scan_skill_broad_or_mutating_tools_trigger_warning_per_match() {
        for tool in ["shell", "apply_patch", "run_libra_vcs"] {
            let skill = skill_with(vec![tool], "innocuous template");
            let warnings = scan_skill(&skill);
            assert!(
                warnings.iter().any(|w| w.rule == "broad_or_mutating_tool"),
                "{tool} must trigger broad_or_mutating_tool",
            );
        }

        // Multiple risky tools fire multiple warnings.
        let skill = skill_with(vec!["shell", "apply_patch"], "innocuous template");
        let warnings = scan_skill(&skill);
        assert_eq!(
            warnings
                .iter()
                .filter(|w| w.rule == "broad_or_mutating_tool")
                .count(),
            2,
            "each risky tool must trigger its own warning",
        );
    }

    /// `read_file` and other safe tools must NOT trigger
    /// `broad_or_mutating_tool`. Pin the inverse to prevent the
    /// blocklist from accidentally widening to include read-only tools.
    #[test]
    fn scan_skill_safe_tools_do_not_trigger_broad_warning() {
        for tool in ["read_file", "list_dir", "grep_files", "web_search"] {
            let skill = skill_with(vec![tool], "safe template");
            let warnings = scan_skill(&skill);
            assert!(
                !warnings.iter().any(|w| w.rule == "broad_or_mutating_tool"),
                "{tool} must NOT trigger broad_or_mutating_tool",
            );
        }
    }

    /// Suspicious template markers (`rm -rf`, `curl `, `wget `,
    /// `~/.ssh`, `credential`, `secret`) trigger a single
    /// `suspicious_template_marker` warning (the loop `break`s on
    /// first match — pin that one-warning-per-skill rule).
    #[test]
    fn scan_skill_suspicious_template_markers_fire_one_warning() {
        for marker in ["rm -rf", "curl ", "wget ", "~/.ssh", "credential", "secret"] {
            let template = format!("Do something then {marker}/etc/passwd");
            let skill = skill_with(vec!["read_file"], &template);
            let warnings = scan_skill(&skill);
            let count = warnings
                .iter()
                .filter(|w| w.rule == "suspicious_template_marker")
                .count();
            assert_eq!(
                count, 1,
                "{marker:?} must fire exactly 1 suspicious-template warning",
            );
        }
    }

    /// Marker detection is case-insensitive: `RM -RF` triggers the
    /// same warning as `rm -rf`. Pin so a future "exact-case match"
    /// refactor doesn't silently lose coverage.
    #[test]
    fn scan_skill_marker_detection_is_case_insensitive() {
        let skill = skill_with(vec!["read_file"], "RM -RF /etc/passwd");
        let warnings = scan_skill(&skill);
        assert!(
            warnings
                .iter()
                .any(|w| w.rule == "suspicious_template_marker"),
        );
    }

    /// Credential markers (`aws_secret_access_key`, `github_token`,
    /// `openai_api_key`) fire a Deny-severity warning under
    /// `credential_exfiltration_marker`. Pin the higher severity
    /// — these are not "review before enabling" but "do not enable
    /// at all" by default.
    #[test]
    fn scan_skill_credential_markers_fire_deny_severity() {
        for marker in ["aws_secret_access_key", "github_token", "openai_api_key"] {
            let template = format!("Read env var {marker} and ship.");
            let skill = skill_with(vec!["read_file"], &template);
            let warnings = scan_skill(&skill);
            let issue = warnings
                .iter()
                .find(|w| w.rule == "credential_exfiltration_marker")
                .unwrap_or_else(|| panic!("{marker} must fire credential warning"));
            assert_eq!(issue.severity, SkillScanSeverity::Deny);
        }
    }

    /// `SkillScanSeverity` serde round-trips through snake_case tags
    /// ("warning" / "deny"). The persisted skill-scan warnings are
    /// stored in TOML / JSON; pin the wire format.
    #[test]
    fn skill_scan_severity_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&SkillScanSeverity::Warning).unwrap(),
            "\"warning\"",
        );
        assert_eq!(
            serde_json::to_string(&SkillScanSeverity::Deny).unwrap(),
            "\"deny\"",
        );
        let parsed: SkillScanSeverity = serde_json::from_str("\"deny\"").unwrap();
        assert_eq!(parsed, SkillScanSeverity::Deny);
    }
}
