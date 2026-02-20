//! Slash command parser: markdown + YAML frontmatter → CommandDefinition.

use std::path::Path;

/// A parsed slash command definition.
#[derive(Debug, Clone)]
pub struct CommandDefinition {
    /// Command name (e.g., "plan", "code-review").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Optional agent to use when executing this command.
    pub agent: Option<String>,
    /// The command template body (markdown). Contains `$ARGUMENTS` placeholder.
    pub template: String,
}

impl CommandDefinition {
    /// Expand the command template with the given arguments.
    pub fn expand(&self, arguments: &str) -> String {
        self.template.replace("$ARGUMENTS", arguments)
    }
}

/// Parse a markdown string with YAML frontmatter into a CommandDefinition.
pub fn parse_command_definition(content: &str) -> Option<CommandDefinition> {
    let content = content.trim();
    if !content.starts_with("---") {
        return None;
    }

    let after_first_fence = &content[3..];
    let end_fence = after_first_fence.find("---")?;
    let frontmatter = after_first_fence[..end_fence].trim();
    let body = after_first_fence[end_fence + 3..].trim();

    let mut name = None;
    let mut description = None;
    let mut agent = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("agent:") {
            let val = val.trim();
            if !val.is_empty() {
                agent = Some(val.to_string());
            }
        }
    }

    Some(CommandDefinition {
        name: name?,
        description: description.unwrap_or_default(),
        agent,
        template: body.to_string(),
    })
}

/// Load a command definition from a file path.
pub fn load_command_from_file(path: &Path) -> Option<CommandDefinition> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read command file");
            return None;
        }
    };
    let result = parse_command_definition(&content);
    if result.is_none() {
        tracing::warn!(path = %path.display(), "failed to parse command definition");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_definition() {
        let content = r#"---
name: plan
description: Create an implementation plan.
agent: planner
---

## /plan $ARGUMENTS

Create a plan for: $ARGUMENTS
"#;
        let cmd = parse_command_definition(content).unwrap();
        assert_eq!(cmd.name, "plan");
        assert_eq!(cmd.description, "Create an implementation plan.");
        assert_eq!(cmd.agent.as_deref(), Some("planner"));
        assert!(cmd.template.contains("$ARGUMENTS"));
    }

    #[test]
    fn test_parse_command_no_agent() {
        let content = "---\nname: verify\ndescription: Run checks.\nagent:\n---\nBody here";
        let cmd = parse_command_definition(content).unwrap();
        assert_eq!(cmd.name, "verify");
        assert!(cmd.agent.is_none());
    }

    #[test]
    fn test_expand_command() {
        let cmd = CommandDefinition {
            name: "plan".to_string(),
            description: String::new(),
            agent: None,
            template: "Plan for: $ARGUMENTS\n\nDetails about $ARGUMENTS".to_string(),
        };

        let expanded = cmd.expand("add user auth");
        assert_eq!(expanded, "Plan for: add user auth\n\nDetails about add user auth");
    }

    #[test]
    fn test_parse_no_frontmatter() {
        assert!(parse_command_definition("No frontmatter").is_none());
    }

    #[test]
    fn test_parse_missing_name() {
        let content = "---\ndescription: test\n---\nbody";
        assert!(parse_command_definition(content).is_none());
    }

    #[test]
    fn test_parse_embedded_commands() {
        let plan = include_str!("embedded/plan.md");
        let cmd = parse_command_definition(plan).unwrap();
        assert_eq!(cmd.name, "plan");
        assert_eq!(cmd.agent.as_deref(), Some("planner"));

        let review = include_str!("embedded/code_review.md");
        let cmd = parse_command_definition(review).unwrap();
        assert_eq!(cmd.name, "code-review");
        assert_eq!(cmd.agent.as_deref(), Some("code_reviewer"));

        let verify = include_str!("embedded/verify.md");
        let cmd = parse_command_definition(verify).unwrap();
        assert_eq!(cmd.name, "verify");
        assert!(cmd.agent.is_none());

        let tdd = include_str!("embedded/tdd.md");
        let cmd = parse_command_definition(tdd).unwrap();
        assert_eq!(cmd.name, "tdd");
        assert!(cmd.agent.is_none());

        let architect = include_str!("embedded/architect.md");
        let cmd = parse_command_definition(architect).unwrap();
        assert_eq!(cmd.name, "architect");
        assert_eq!(cmd.agent.as_deref(), Some("architect"));

        let build_fix = include_str!("embedded/build_fix.md");
        let cmd = parse_command_definition(build_fix).unwrap();
        assert_eq!(cmd.name, "build-fix");
        assert_eq!(cmd.agent.as_deref(), Some("build_error_resolver"));
    }
}
