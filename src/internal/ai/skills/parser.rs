//! Skill parser: markdown + TOML frontmatter -> [`SkillDefinition`].

use std::{collections::BTreeMap, fmt, path::PathBuf};

use ring::digest::{SHA256, digest};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::scanner::{SkillScanWarning, scan_skill};

/// Parsed markdown skill definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub allowed_tools: Vec<String>,
    pub template: String,
    pub checksum: String,
    pub source_path: Option<PathBuf>,
    pub warnings: Vec<SkillScanWarning>,
}

impl SkillDefinition {
    /// Render the skill body with a small Handlebars-style variable subset.
    ///
    /// Supported values:
    /// - `{{arguments}}` / `{{ARGUMENTS}}` — raw tail after `/skill <name>`
    /// - `{{key}}` — values parsed from `key=value` tokens in the tail
    ///
    /// Unknown placeholders are left intact so long reference templates remain
    /// debuggable instead of silently dropping content.
    pub fn render(&self, arguments: &str) -> String {
        let values = parse_argument_values(arguments);
        render_template(&self.template, arguments, &values)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillFrontmatter {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, rename = "allowed-tools", alias = "allowed_tools")]
    allowed_tools: Vec<String>,
}

/// Skill parse failure with user-readable context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    InvalidToml(String),
    MissingName,
}

impl fmt::Display for SkillParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => formatter.write_str("skill is missing TOML frontmatter"),
            Self::UnterminatedFrontmatter => {
                formatter.write_str("skill frontmatter is missing closing --- fence")
            }
            Self::InvalidToml(error) => {
                write!(formatter, "skill frontmatter is invalid TOML: {error}")
            }
            Self::MissingName => formatter.write_str("skill frontmatter must include `name`"),
        }
    }
}

/// Parse a markdown string with TOML frontmatter into a skill.
pub fn parse_skill_definition(content: &str) -> Result<SkillDefinition, SkillParseError> {
    let content = content.trim();
    if !content.starts_with("---") {
        return Err(SkillParseError::MissingFrontmatter);
    }

    let after_first_fence = &content[3..];
    let end_fence = after_first_fence
        .find("---")
        .ok_or(SkillParseError::UnterminatedFrontmatter)?;
    let frontmatter = after_first_fence[..end_fence].trim();
    let body = after_first_fence[end_fence + 3..].trim();
    let frontmatter: SkillFrontmatter = toml::from_str(frontmatter)
        .map_err(|error| SkillParseError::InvalidToml(error.to_string()))?;
    if frontmatter.name.trim().is_empty() {
        return Err(SkillParseError::MissingName);
    }

    let checksum = hex::encode(digest(&SHA256, content.as_bytes()).as_ref());
    let mut skill = SkillDefinition {
        name: frontmatter.name.trim().to_string(),
        description: frontmatter.description,
        version: frontmatter.version,
        allowed_tools: frontmatter.allowed_tools,
        template: body.to_string(),
        checksum,
        source_path: None,
        warnings: Vec::new(),
    };
    skill.warnings = scan_skill(&skill);
    Ok(skill)
}

pub fn parse_argument_values(arguments: &str) -> BTreeMap<String, String> {
    arguments
        .split_whitespace()
        .filter_map(|token| {
            let (key, value) = token.split_once('=')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some((
                key.to_string(),
                value.trim_matches('"').trim_matches('\'').to_string(),
            ))
        })
        .collect()
}

fn render_template(template: &str, arguments: &str, values: &BTreeMap<String, String>) -> String {
    let mut output = template
        .replace("{{arguments}}", arguments)
        .replace("{{ARGUMENTS}}", arguments)
        .replace("$ARGUMENTS", arguments);
    for (key, value) in values {
        output = output.replace(&format!("{{{{{key}}}}}"), value);
    }
    output
}

pub fn render_skill_metadata_json(skill: &SkillDefinition) -> Value {
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(skill.name.clone()));
    object.insert(
        "checksum".to_string(),
        Value::String(skill.checksum.clone()),
    );
    if let Some(version) = skill.version.as_ref() {
        object.insert("version".to_string(), Value::String(version.clone()));
    }
    object.insert(
        "allowed_tools".to_string(),
        Value::Array(
            skill
                .allowed_tools
                .iter()
                .map(|tool| Value::String(tool.clone()))
                .collect(),
        ),
    );
    Value::Object(object)
}
