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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_parse_error_display_pins_each_variant() {
        assert_eq!(
            SkillParseError::MissingFrontmatter.to_string(),
            "skill is missing TOML frontmatter",
        );
        assert_eq!(
            SkillParseError::UnterminatedFrontmatter.to_string(),
            "skill frontmatter is missing closing --- fence",
        );
        assert_eq!(
            SkillParseError::InvalidToml("expected `=` after key".to_string()).to_string(),
            "skill frontmatter is invalid TOML: expected `=` after key",
        );
        assert_eq!(
            SkillParseError::MissingName.to_string(),
            "skill frontmatter must include `name`",
        );
    }

    /// Happy-path parse: minimal valid frontmatter + body produces
    /// a populated SkillDefinition with the canonical body trimmed.
    #[test]
    fn parse_skill_definition_minimal_happy_path() {
        let raw = "---\nname = \"hello\"\n---\nHello {{arguments}}.";
        let skill = parse_skill_definition(raw).expect("valid skill");
        assert_eq!(skill.name, "hello");
        assert_eq!(skill.description, "");
        assert!(skill.version.is_none());
        assert!(skill.allowed_tools.is_empty());
        assert_eq!(skill.template, "Hello {{arguments}}.");
        assert!(!skill.checksum.is_empty());
    }

    /// `parse_skill_definition` rejects content without leading
    /// `---` with `MissingFrontmatter`.
    #[test]
    fn parse_skill_definition_rejects_missing_frontmatter() {
        let err = parse_skill_definition("just body, no fence").unwrap_err();
        assert_eq!(err, SkillParseError::MissingFrontmatter);
    }

    /// A leading `---` without a closing fence produces
    /// `UnterminatedFrontmatter`.
    #[test]
    fn parse_skill_definition_rejects_unterminated_frontmatter() {
        let raw = "---\nname = \"hello\"\nbody";
        let err = parse_skill_definition(raw).unwrap_err();
        assert_eq!(err, SkillParseError::UnterminatedFrontmatter);
    }

    /// Malformed TOML in the frontmatter surfaces as `InvalidToml`
    /// with the underlying parser message embedded.
    #[test]
    fn parse_skill_definition_invalid_toml_surfaces_inner_error() {
        let raw = "---\nname = \nstray = value\n---\nbody";
        match parse_skill_definition(raw) {
            Err(SkillParseError::InvalidToml(msg)) => {
                assert!(
                    !msg.is_empty(),
                    "InvalidToml must carry the underlying message",
                );
            }
            other => panic!("expected InvalidToml, got {other:?}"),
        }
    }

    /// Empty / whitespace-only `name` triggers `MissingName`.
    #[test]
    fn parse_skill_definition_empty_name_triggers_missing_name() {
        for raw in [
            "---\nname = \"\"\n---\nbody",
            "---\nname = \"   \"\n---\nbody",
        ] {
            let err = parse_skill_definition(raw).unwrap_err();
            assert_eq!(err, SkillParseError::MissingName, "raw {raw:?}");
        }
    }

    /// Frontmatter accepts both `allowed-tools` (canonical) and
    /// `allowed_tools` (alias). Pin both forms so a future serde
    /// refactor can't silently drop the kebab-case form.
    #[test]
    fn parse_skill_definition_accepts_allowed_tools_kebab_and_snake_case() {
        let kebab = "---\nname = \"k\"\nallowed-tools = [\"read_file\"]\n---\nbody";
        let snake = "---\nname = \"s\"\nallowed_tools = [\"read_file\"]\n---\nbody";
        for raw in [kebab, snake] {
            let skill = parse_skill_definition(raw).expect("valid");
            assert_eq!(skill.allowed_tools, vec!["read_file".to_string()]);
        }
    }

    /// The body section is everything after the closing `---` fence,
    /// trimmed. Pins the trim direction.
    #[test]
    fn parse_skill_definition_trims_body_after_fence() {
        let raw = "---\nname = \"x\"\n---\n\n  inner body  \n\n";
        let skill = parse_skill_definition(raw).expect("valid");
        assert_eq!(skill.template, "inner body");
    }

    /// `parse_skill_definition` produces a stable SHA256 checksum
    /// of the trimmed content: identical inputs produce identical
    /// checksums.
    #[test]
    fn parse_skill_definition_checksum_is_stable_across_repeated_parses() {
        let raw = "---\nname = \"stable\"\n---\nBody";
        let a = parse_skill_definition(raw).expect("valid");
        let b = parse_skill_definition(raw).expect("valid");
        assert_eq!(a.checksum, b.checksum);
        // SHA256 hex is 64 chars.
        assert_eq!(a.checksum.len(), 64);
    }

    /// `parse_argument_values` extracts `key=value` tokens, trims
    /// surrounding double + single quotes from the value, and ignores
    /// tokens without `=`.
    #[test]
    fn parse_argument_values_extracts_key_value_pairs_and_strips_quotes() {
        let values = parse_argument_values(r#"name=foo path="src/main.rs" empty quote='bar'"#);
        assert_eq!(values.get("name").map(String::as_str), Some("foo"));
        assert_eq!(values.get("path").map(String::as_str), Some("src/main.rs"));
        assert_eq!(values.get("quote").map(String::as_str), Some("bar"));
        // Tokens without `=` are skipped.
        assert!(!values.contains_key("empty"));
    }

    /// `parse_argument_values` ignores tokens whose key would be
    /// empty after trim (`=value`, ` =value`).
    #[test]
    fn parse_argument_values_skips_empty_keys() {
        let values = parse_argument_values("=oops value=ok");
        assert!(!values.values().any(|v| v == "oops"));
        assert_eq!(values.get("value").map(String::as_str), Some("ok"));
    }

    /// `render_template` substitutes `{{arguments}}`, `{{ARGUMENTS}}`,
    /// and `$ARGUMENTS` with the supplied raw tail, plus any
    /// `{{key}}` placeholders backed by parsed key=value tokens.
    #[test]
    fn render_template_substitutes_all_argument_forms_and_named_values() {
        let mut values = BTreeMap::new();
        values.insert("name".to_string(), "Eli".to_string());
        let template = "args={{arguments}} ARGS={{ARGUMENTS}} dollar=$ARGUMENTS name={{name}}";
        let out = render_template(template, "raw tail", &values);
        assert_eq!(out, "args=raw tail ARGS=raw tail dollar=raw tail name=Eli");
    }

    /// `render_template` leaves unknown `{{key}}` placeholders
    /// untouched so reference templates with optional fields stay
    /// debuggable instead of silently dropping content.
    #[test]
    fn render_template_leaves_unknown_placeholders_intact() {
        let values = BTreeMap::new();
        let out = render_template("hello {{unknown}}", "tail", &values);
        assert_eq!(out, "hello {{unknown}}");
    }

    /// `render_skill_metadata_json` includes `name`/`checksum`/
    /// `allowed_tools` always and omits `version` when `None`.
    #[test]
    fn render_skill_metadata_json_omits_missing_optional_version() {
        let raw = "---\nname = \"x\"\nallowed-tools = [\"read_file\"]\n---\nbody";
        let skill = parse_skill_definition(raw).expect("valid");
        let meta = render_skill_metadata_json(&skill);
        let object = meta.as_object().expect("object");
        assert!(object.contains_key("name"));
        assert!(object.contains_key("checksum"));
        assert!(object.contains_key("allowed_tools"));
        assert!(
            !object.contains_key("version"),
            "version must be omitted when None; got {meta:?}",
        );
    }

    /// `render_skill_metadata_json` includes `version` when populated.
    #[test]
    fn render_skill_metadata_json_includes_version_when_present() {
        let raw = "---\nname = \"v\"\nversion = \"1.2.3\"\n---\nbody";
        let skill = parse_skill_definition(raw).expect("valid");
        let meta = render_skill_metadata_json(&skill);
        assert_eq!(meta.get("version").and_then(Value::as_str), Some("1.2.3"),);
    }
}
