//! System prompt builder that composes rules into a final prompt string.

use std::path::{Path, PathBuf};

use super::{
    context::ContextMode,
    loader::load_all_rules,
    rules::{RuleCategory, RuleFile},
};

/// Builds a complete system prompt from modular rule files.
///
/// Rules are loaded from a three-tier hierarchy (project-local > user-global > embedded)
/// and composed in a fixed order. The `{working_dir}` placeholder in rule content is
/// substituted with the actual working directory path.
pub struct SystemPromptBuilder {
    working_dir: PathBuf,
    rules: Vec<RuleFile>,
    context: Option<ContextMode>,
    extra_sections: Vec<(String, String)>,
}

impl SystemPromptBuilder {
    /// Create a new builder that loads all default rules for the given working directory.
    pub fn new(working_dir: &Path) -> Self {
        let rules = load_all_rules(working_dir);
        Self {
            working_dir: working_dir.to_path_buf(),
            rules,
            context: None,
            extra_sections: Vec::new(),
        }
    }

    /// Replace the content of a specific rule category.
    pub fn override_rule(mut self, category: RuleCategory, content: impl Into<String>) -> Self {
        if let Some(rule) = self.rules.iter_mut().find(|r| r.category == category) {
            rule.content = content.into();
        }
        self
    }

    /// Set the operating context mode (dev, review, research).
    ///
    /// The context is appended after all rules, adjusting the agent's
    /// behavior and priorities for the given mode.
    pub fn with_context(mut self, mode: ContextMode) -> Self {
        self.context = Some(mode);
        self
    }

    /// Append a custom section to the end of the prompt.
    pub fn extra_section(mut self, heading: impl Into<String>, content: impl Into<String>) -> Self {
        self.extra_sections.push((heading.into(), content.into()));
        self
    }

    /// Build the final system prompt string.
    pub fn build(self) -> String {
        let working_dir_str = self.working_dir.display().to_string();
        let mut parts: Vec<String> = Vec::with_capacity(
            self.rules.len() + self.extra_sections.len() + usize::from(self.context.is_some()),
        );

        for rule in &self.rules {
            let content = rule.content.replace("{working_dir}", &working_dir_str);
            parts.push(content);
        }

        if let Some(ref context) = self.context {
            let content = context.load_content(&self.working_dir);
            parts.push(content.replace("{working_dir}", &working_dir_str));
        }

        for (heading, content) in &self.extra_sections {
            let section = format!("## {}\n\n{}", heading, content);
            parts.push(section.replace("{working_dir}", &working_dir_str));
        }

        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_build_contains_base_content() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path()).build();
        assert!(prompt.contains("Libra"), "prompt should contain Libra identity");
        assert!(prompt.contains("coding assistant"), "prompt should contain role description");
    }

    #[test]
    fn test_working_dir_substituted() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path()).build();
        let dir_str = tmp.path().display().to_string();
        assert!(
            prompt.contains(&dir_str),
            "prompt should contain the actual working directory path"
        );
        assert!(
            !prompt.contains("{working_dir}"),
            "prompt should not contain raw placeholder"
        );
    }

    #[test]
    fn test_all_rule_sections_present() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path()).build();

        assert!(prompt.contains("Coding Style"), "missing coding style section");
        assert!(prompt.contains("Error Handling"), "missing error handling section");
        assert!(prompt.contains("Git Workflow"), "missing git workflow section");
        assert!(prompt.contains("Testing"), "missing testing section");
        assert!(prompt.contains("Security"), "missing security section");
        assert!(prompt.contains("Tool Use"), "missing tool use section");
    }

    #[test]
    fn test_override_rule() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path())
            .override_rule(RuleCategory::Security, "Custom security rules here.")
            .build();

        assert!(prompt.contains("Custom security rules here."));
    }

    #[test]
    fn test_extra_section_appended() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path())
            .extra_section("Project Context", "This is a Rust CLI tool.")
            .build();

        assert!(prompt.contains("## Project Context"));
        assert!(prompt.contains("This is a Rust CLI tool."));
    }

    #[test]
    fn test_extra_section_working_dir_substituted() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path())
            .extra_section("Extra", "Path is {working_dir}")
            .build();

        let dir_str = tmp.path().display().to_string();
        assert!(prompt.contains(&format!("Path is {}", dir_str)));
    }

    #[test]
    fn test_prompt_is_nontrivial_length() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path()).build();
        // The composed prompt should be substantial (all 7 rule files)
        assert!(
            prompt.len() > 1000,
            "composed prompt should be substantial, got {} bytes",
            prompt.len()
        );
    }

    #[test]
    fn test_project_local_override() {
        let tmp = TempDir::new().unwrap();
        let rules_dir = tmp.path().join(".libra").join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(
            rules_dir.join("base.md"),
            "You are a custom assistant for ProjectX.\n\nWorking directory: {working_dir}",
        )
        .unwrap();

        let prompt = SystemPromptBuilder::new(tmp.path()).build();
        assert!(prompt.contains("custom assistant for ProjectX"));
        assert!(prompt.contains(&tmp.path().display().to_string()));
    }

    #[test]
    fn test_with_context_dev() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path())
            .with_context(ContextMode::Dev)
            .build();

        assert!(prompt.contains("Development Mode"));
    }

    #[test]
    fn test_with_context_review() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path())
            .with_context(ContextMode::Review)
            .build();

        assert!(prompt.contains("Code Review Mode"));
    }

    #[test]
    fn test_with_context_research() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path())
            .with_context(ContextMode::Research)
            .build();

        assert!(prompt.contains("Research Mode"));
    }

    #[test]
    fn test_context_appears_after_rules() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path())
            .with_context(ContextMode::Dev)
            .build();

        let tool_use_pos = prompt.find("Tool Use").expect("should contain Tool Use");
        let context_pos = prompt.find("Development Mode").expect("should contain context");
        assert!(context_pos > tool_use_pos, "context should appear after rules");
    }

    #[test]
    fn test_no_context_by_default() {
        let tmp = TempDir::new().unwrap();
        let prompt = SystemPromptBuilder::new(tmp.path()).build();

        assert!(!prompt.contains("Development Mode"));
        assert!(!prompt.contains("Code Review Mode"));
        assert!(!prompt.contains("Research Mode"));
    }
}
