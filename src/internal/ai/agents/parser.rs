//! Agent definition parser: markdown + YAML frontmatter → AgentDefinition.

use std::path::Path;

/// A parsed agent definition from a markdown file with YAML frontmatter.
#[derive(Debug, Clone)]
pub struct AgentDefinition {
    /// Unique name for this agent.
    pub name: String,
    /// Human-readable description (used for auto-selection matching).
    pub description: String,
    /// List of tool names this agent is allowed to use.
    pub tools: Vec<String>,
    /// Model preference (e.g., "default", "fast", "powerful").
    pub model_preference: String,
    /// The system prompt body (everything after the frontmatter).
    pub system_prompt: String,
}

/// Parse a markdown string with YAML frontmatter into an AgentDefinition.
///
/// Expected format:
/// ```text
/// ---
/// name: planner
/// description: Implementation planning specialist...
/// tools: ["read_file", "list_dir", "grep_files"]
/// model: default
/// ---
///
/// You are an implementation planner...
/// ```
pub fn parse_agent_definition(content: &str) -> Option<AgentDefinition> {
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
    let mut tools = Vec::new();
    let mut model_preference = "default".to_string();

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("model:") {
            model_preference = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("tools:") {
            tools = parse_string_list(val.trim());
        }
    }

    Some(AgentDefinition {
        name: name?,
        description: description.unwrap_or_default(),
        tools,
        model_preference,
        system_prompt: body.to_string(),
    })
}

/// Load an agent definition from a file path.
pub fn load_agent_from_file(path: &Path) -> Option<AgentDefinition> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read agent file");
            return None;
        }
    };
    let result = parse_agent_definition(&content);
    if result.is_none() {
        tracing::warn!(path = %path.display(), "failed to parse agent definition");
    }
    result
}

/// Parse a YAML-style string list: `["a", "b", "c"]` → Vec<String>.
fn parse_string_list(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = s.strip_prefix('[').unwrap_or(s);
    let s = s.strip_suffix(']').unwrap_or(s);
    s.split(',')
        .map(|item| {
            item.trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_AGENT: &str = r#"---
name: planner
description: Implementation planning specialist
tools: ["read_file", "list_dir", "grep_files"]
model: default
---

You are an implementation planner.

## Planning Process

1. Understand requirements
2. Explore codebase
"#;

    #[test]
    fn test_parse_agent_definition() {
        let def = parse_agent_definition(SAMPLE_AGENT).unwrap();
        assert_eq!(def.name, "planner");
        assert_eq!(def.description, "Implementation planning specialist");
        assert_eq!(def.tools, vec!["read_file", "list_dir", "grep_files"]);
        assert_eq!(def.model_preference, "default");
        assert!(def.system_prompt.contains("implementation planner"));
    }

    #[test]
    fn test_parse_no_frontmatter() {
        assert!(parse_agent_definition("No frontmatter here").is_none());
    }

    #[test]
    fn test_parse_missing_name() {
        let content = "---\ndescription: test\n---\nbody";
        assert!(parse_agent_definition(content).is_none());
    }

    #[test]
    fn test_parse_string_list() {
        assert_eq!(
            parse_string_list(r#"["a", "b", "c"]"#),
            vec!["a", "b", "c"]
        );
        assert_eq!(parse_string_list("[]"), Vec::<String>::new());
        assert_eq!(parse_string_list(r#"["single"]"#), vec!["single"]);
    }

    #[test]
    fn test_parse_embedded_agents() {
        let planner = include_str!("embedded/planner.md");
        let def = parse_agent_definition(planner).unwrap();
        assert_eq!(def.name, "planner");

        let reviewer = include_str!("embedded/code_reviewer.md");
        let def = parse_agent_definition(reviewer).unwrap();
        assert_eq!(def.name, "code_reviewer");

        let architect = include_str!("embedded/architect.md");
        let def = parse_agent_definition(architect).unwrap();
        assert_eq!(def.name, "architect");

        let resolver = include_str!("embedded/build_error_resolver.md");
        let def = parse_agent_definition(resolver).unwrap();
        assert_eq!(def.name, "build_error_resolver");
    }
}
