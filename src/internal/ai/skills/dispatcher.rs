//! Skill dispatcher for `/skill <name> ...` invocations.
//!
//! `/skill <name> ...` 调用的技能调度程序。

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::skills::parser::parse_skill_definition;

    #[test]
    fn skill_dispatch_error_display_pins_each_variant() {
        assert_eq!(
            SkillDispatchError::MissingName.to_string(),
            "Usage: /skill <name> [key=value ...]",
        );
        assert_eq!(
            SkillDispatchError::UnknownSkill("review".to_string()).to_string(),
            "Unknown skill `review`.",
        );
    }

    fn dispatcher_with(skills: Vec<&str>) -> SkillDispatcher {
        let defs: Vec<SkillDefinition> = skills
            .into_iter()
            .map(|raw| parse_skill_definition(raw).expect("valid skill"))
            .collect();
        SkillDispatcher::new(defs)
    }

    /// `SkillDispatcher::default()` produces an empty in-memory table
    /// that exposes no skills.
    #[test]
    fn dispatcher_default_is_empty() {
        let dispatcher = SkillDispatcher::default();
        assert!(dispatcher.skills().is_empty());
        assert!(dispatcher.get("anything").is_none());
    }

    /// `get` returns `Some` for a registered name and `None` for an
    /// unregistered one.
    #[test]
    fn dispatcher_get_returns_some_for_known_name_none_otherwise() {
        let dispatcher = dispatcher_with(vec![
            "---\nname = \"alpha\"\n---\nBody A",
            "---\nname = \"beta\"\n---\nBody B",
        ]);
        assert!(dispatcher.get("alpha").is_some());
        assert!(dispatcher.get("beta").is_some());
        assert!(dispatcher.get("gamma").is_none());
        assert!(
            dispatcher.get("ALPHA").is_none(),
            "lookup is case-sensitive"
        );
    }

    /// Empty / whitespace-only args → MissingName.
    #[test]
    fn dispatcher_dispatch_empty_args_yields_missing_name() {
        let dispatcher = dispatcher_with(vec!["---\nname = \"x\"\n---\nBody"]);
        for raw in ["", "   ", "\t\n"] {
            let err = dispatcher.dispatch(raw).unwrap_err();
            assert_eq!(err, SkillDispatchError::MissingName);
        }
    }

    /// Unknown skill name → UnknownSkill(name) carrying the offending
    /// name for the error message.
    #[test]
    fn dispatcher_dispatch_unknown_skill_carries_name_in_error() {
        let dispatcher = dispatcher_with(vec!["---\nname = \"alpha\"\n---\nBody"]);
        let err = dispatcher.dispatch("unknown-skill some args").unwrap_err();
        match err {
            SkillDispatchError::UnknownSkill(name) => {
                assert_eq!(name, "unknown-skill");
            }
            other => panic!("expected UnknownSkill, got {other:?}"),
        }
    }

    /// Happy path: name alone (no args) renders the template, threads
    /// allowed_tools and populates metadata JSON.
    #[test]
    fn dispatcher_dispatch_name_only_renders_template_and_metadata() {
        let dispatcher = dispatcher_with(vec![
            "---\nname = \"greet\"\nallowed-tools = [\"read_file\"]\n---\nHello",
        ]);
        let result = dispatcher.dispatch("greet").expect("happy path");
        assert_eq!(result.prompt, "Hello");
        assert_eq!(result.allowed_tools, vec!["read_file".to_string()]);
        let meta = result.metadata.as_object().expect("metadata is object");
        assert!(meta.contains_key("name"));
        assert!(meta.contains_key("checksum"));
        assert!(meta.contains_key("allowed_tools"));
    }

    /// `dispatch` with `name + raw tail` substitutes `{{arguments}}`
    /// and key=value tokens. Pin the args-routing contract.
    #[test]
    fn dispatcher_dispatch_substitutes_arguments_and_named_values() {
        let dispatcher = dispatcher_with(vec![
            "---\nname = \"render\"\n---\nargs={{arguments}} path={{path}}",
        ]);
        let result = dispatcher
            .dispatch("render path=src/main.rs --flag")
            .expect("happy path");
        // The raw tail is `path=src/main.rs --flag` (after stripping
        // leading whitespace).
        assert!(
            result.prompt.contains("args=path=src/main.rs --flag"),
            "args= must carry the raw tail; got {}",
            result.prompt,
        );
        // {{path}} substitution from the key=value token.
        assert!(
            result.prompt.contains("path=src/main.rs"),
            "named-value substitution failed; got {}",
            result.prompt,
        );
    }

    /// Whitespace between name and tail is trimmed.
    #[test]
    fn dispatcher_dispatch_trims_whitespace_between_name_and_tail() {
        let dispatcher = dispatcher_with(vec!["---\nname = \"trim\"\n---\ntail=[{{arguments}}]"]);
        let result = dispatcher.dispatch("trim    spaced    tail").expect("ok");
        // Internal whitespace is preserved in the args; only the
        // leading separator is trimmed.
        assert!(
            result.prompt.contains("tail=[spaced    tail]"),
            "got {}",
            result.prompt,
        );
    }

    /// `skills()` returns the live slice; `Default` produces an
    /// empty Vec.
    #[test]
    fn dispatcher_skills_returns_borrowed_slice() {
        let dispatcher = dispatcher_with(vec![
            "---\nname = \"alpha\"\n---\nA",
            "---\nname = \"beta\"\n---\nB",
        ]);
        let slice: &[SkillDefinition] = dispatcher.skills();
        assert_eq!(slice.len(), 2);
        assert_eq!(slice[0].name, "alpha");
        assert_eq!(slice[1].name, "beta");
    }
}
