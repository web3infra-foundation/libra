//! Skill dispatcher for `/skill <name> ...` invocations.

use super::{SkillDefinition, parser::render_skill_metadata_json};

/// Result of activating a skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDispatchResult {
    pub prompt: String,
    pub allowed_tools: Vec<String>,
    pub metadata: serde_json::Value,
}

/// In-memory skill lookup table.
#[derive(Debug, Clone, Default)]
pub struct SkillDispatcher {
    skills: Vec<SkillDefinition>,
}

impl SkillDispatcher {
    pub fn new(skills: Vec<SkillDefinition>) -> Self {
        Self { skills }
    }

    pub fn skills(&self) -> &[SkillDefinition] {
        &self.skills
    }

    pub fn get(&self, name: &str) -> Option<&SkillDefinition> {
        self.skills.iter().find(|skill| skill.name == name)
    }

    pub fn dispatch(&self, args: &str) -> Result<SkillDispatchResult, SkillDispatchError> {
        let args = args.trim();
        if args.is_empty() {
            return Err(SkillDispatchError::MissingName);
        }
        let (name, arguments) = args
            .split_once(char::is_whitespace)
            .map(|(name, rest)| (name, rest.trim()))
            .unwrap_or((args, ""));
        let skill = self
            .get(name)
            .ok_or_else(|| SkillDispatchError::UnknownSkill(name.to_string()))?;
        Ok(SkillDispatchResult {
            prompt: skill.render(arguments),
            allowed_tools: skill.allowed_tools.clone(),
            metadata: render_skill_metadata_json(skill),
        })
    }
}

/// User-readable skill dispatch errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillDispatchError {
    MissingName,
    UnknownSkill(String),
}

impl std::fmt::Display for SkillDispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingName => formatter.write_str("Usage: /skill <name> [key=value ...]"),
            Self::UnknownSkill(name) => write!(formatter, "Unknown skill `{name}`."),
        }
    }
}
