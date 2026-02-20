//! Rule category types and rule file structures for the prompt system.

use std::fmt;

/// Categories of rules that compose the system prompt.
///
/// Each category maps to an embedded markdown file that can be overridden
/// by project-local or user-global rule files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleCategory {
    /// Core identity, behavioral guidelines, working directory.
    Base,
    /// Rust conventions: naming, imports, file organization.
    CodingStyle,
    /// Error propagation, thiserror patterns, logging.
    ErrorHandling,
    /// Conventional commits, PR workflow, branch strategy.
    GitWorkflow,
    /// TDD cycle, coverage targets, test patterns.
    Testing,
    /// Input validation, path sandboxing, secret management.
    Security,
    /// Tool usage guidelines for read_file, list_dir, grep_files, apply_patch.
    ToolUse,
}

impl RuleCategory {
    /// Returns all categories in the order they should appear in the prompt.
    pub fn all_in_order() -> &'static [RuleCategory] {
        &[
            RuleCategory::Base,
            RuleCategory::CodingStyle,
            RuleCategory::ErrorHandling,
            RuleCategory::GitWorkflow,
            RuleCategory::Testing,
            RuleCategory::Security,
            RuleCategory::ToolUse,
        ]
    }

    /// Returns the filename (without extension) for this category.
    pub fn filename(&self) -> &'static str {
        match self {
            RuleCategory::Base => "base",
            RuleCategory::CodingStyle => "coding_style",
            RuleCategory::ErrorHandling => "error_handling",
            RuleCategory::GitWorkflow => "git_workflow",
            RuleCategory::Testing => "testing",
            RuleCategory::Security => "security",
            RuleCategory::ToolUse => "tool_use",
        }
    }

    /// Returns the embedded default content for this category.
    pub fn embedded_content(&self) -> &'static str {
        match self {
            RuleCategory::Base => include_str!("embedded/base.md"),
            RuleCategory::CodingStyle => include_str!("embedded/coding_style.md"),
            RuleCategory::ErrorHandling => include_str!("embedded/error_handling.md"),
            RuleCategory::GitWorkflow => include_str!("embedded/git_workflow.md"),
            RuleCategory::Testing => include_str!("embedded/testing.md"),
            RuleCategory::Security => include_str!("embedded/security.md"),
            RuleCategory::ToolUse => include_str!("embedded/tool_use.md"),
        }
    }
}

impl fmt::Display for RuleCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.filename())
    }
}

/// A loaded rule file with its category and content.
#[derive(Debug, Clone)]
pub struct RuleFile {
    /// The category this rule belongs to.
    pub category: RuleCategory,
    /// The markdown content of the rule.
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_in_order_returns_all_variants() {
        let all = RuleCategory::all_in_order();
        assert_eq!(all.len(), 7);
        assert_eq!(all[0], RuleCategory::Base);
        assert_eq!(all[6], RuleCategory::ToolUse);
    }

    #[test]
    fn test_filename_mapping() {
        assert_eq!(RuleCategory::Base.filename(), "base");
        assert_eq!(RuleCategory::CodingStyle.filename(), "coding_style");
        assert_eq!(RuleCategory::ErrorHandling.filename(), "error_handling");
        assert_eq!(RuleCategory::GitWorkflow.filename(), "git_workflow");
        assert_eq!(RuleCategory::Testing.filename(), "testing");
        assert_eq!(RuleCategory::Security.filename(), "security");
        assert_eq!(RuleCategory::ToolUse.filename(), "tool_use");
    }

    #[test]
    fn test_embedded_content_is_nonempty() {
        for category in RuleCategory::all_in_order() {
            let content = category.embedded_content();
            assert!(
                !content.is_empty(),
                "embedded content for {:?} should not be empty",
                category
            );
        }
    }

    #[test]
    fn test_base_contains_working_dir_placeholder() {
        let content = RuleCategory::Base.embedded_content();
        assert!(
            content.contains("{working_dir}"),
            "base.md must contain {{working_dir}} placeholder"
        );
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", RuleCategory::CodingStyle), "coding_style");
    }
}
