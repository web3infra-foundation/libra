//! Three-tier rule loader: project-local > user-global > embedded defaults.

use std::path::Path;

use tracing::debug;

use super::rules::{RuleCategory, RuleFile};

/// Load a single rule, checking overrides in priority order:
///
/// 1. `{working_dir}/.libra/rules/{category}.md` (project-local override)
/// 2. `~/.config/libra/rules/{category}.md` (user-global override)
/// 3. Embedded default compiled into the binary
pub fn load_rule(category: RuleCategory, working_dir: &Path) -> RuleFile {
    let filename = format!("{}.md", category.filename());

    // 1. Project-local override
    let project_path = working_dir.join(".libra").join("rules").join(&filename);
    if let Some(content) = read_non_empty(&project_path) {
        debug!(category = %category, path = %project_path.display(), "loaded project-local rule override");
        return RuleFile {
            category,
            content,
        };
    }

    // 2. User-global override
    if let Some(config_dir) = dirs::config_dir() {
        let user_path = config_dir.join("libra").join("rules").join(&filename);
        if let Some(content) = read_non_empty(&user_path) {
            debug!(category = %category, path = %user_path.display(), "loaded user-global rule override");
            return RuleFile {
                category,
                content,
            };
        }
    }

    // 3. Embedded default
    debug!(category = %category, "using embedded default rule");
    RuleFile {
        category,
        content: category.embedded_content().to_string(),
    }
}

/// Load all rules in prompt composition order.
pub fn load_all_rules(working_dir: &Path) -> Vec<RuleFile> {
    RuleCategory::all_in_order()
        .iter()
        .map(|&cat| load_rule(cat, working_dir))
        .collect()
}

/// Read a file and return its content if it exists and is non-empty.
fn read_non_empty(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => Some(content),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_rule_returns_embedded_default() {
        let tmp = TempDir::new().unwrap();
        let rule = load_rule(RuleCategory::Base, tmp.path());
        assert_eq!(rule.category, RuleCategory::Base);
        assert!(!rule.content.is_empty());
        assert!(rule.content.contains("{working_dir}"));
    }

    #[test]
    fn test_load_all_rules_returns_all_categories() {
        let tmp = TempDir::new().unwrap();
        let rules = load_all_rules(tmp.path());
        assert_eq!(rules.len(), RuleCategory::all_in_order().len());
        for (rule, &expected_cat) in rules.iter().zip(RuleCategory::all_in_order()) {
            assert_eq!(rule.category, expected_cat);
        }
    }

    #[test]
    fn test_project_local_override_takes_precedence() {
        let tmp = TempDir::new().unwrap();
        let rules_dir = tmp.path().join(".libra").join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("base.md"), "Custom base rule content").unwrap();

        let rule = load_rule(RuleCategory::Base, tmp.path());
        assert_eq!(rule.content, "Custom base rule content");
    }

    #[test]
    fn test_empty_override_falls_back_to_embedded() {
        let tmp = TempDir::new().unwrap();
        let rules_dir = tmp.path().join(".libra").join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("base.md"), "   \n  ").unwrap();

        let rule = load_rule(RuleCategory::Base, tmp.path());
        // Should fall back to embedded since override is whitespace-only
        assert!(rule.content.contains("{working_dir}"));
    }

    #[test]
    fn test_all_embedded_rules_load_without_panic() {
        let tmp = TempDir::new().unwrap();
        for &category in RuleCategory::all_in_order() {
            let rule = load_rule(category, tmp.path());
            assert!(!rule.content.is_empty(), "{:?} should have content", category);
        }
    }
}
