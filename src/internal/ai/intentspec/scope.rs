use std::collections::BTreeSet;

use super::types::Intent;

pub fn effective_write_scope(intent: &Intent) -> Vec<String> {
    let mut patterns = BTreeSet::new();

    if let Some(touch_hints) = &intent.touch_hints {
        collect_patterns(&mut patterns, &touch_hints.files);
    }

    collect_patterns(
        &mut patterns,
        intent
            .in_scope
            .iter()
            .filter(|item| looks_like_path_pattern(item))
            .map(String::as_str),
    );

    patterns.into_iter().collect()
}

pub fn effective_forbidden_scope(intent: &Intent) -> Vec<String> {
    let mut patterns = BTreeSet::new();
    collect_patterns(
        &mut patterns,
        intent
            .out_of_scope
            .iter()
            .filter(|item| looks_like_path_pattern(item))
            .map(String::as_str),
    );
    patterns.into_iter().collect()
}

fn collect_patterns<S>(out: &mut BTreeSet<String>, items: impl IntoIterator<Item = S>)
where
    S: AsRef<str>,
{
    for item in items {
        if let Some(pattern) = normalize_scope_pattern(item.as_ref()) {
            out.insert(pattern);
        }
    }
}

fn normalize_scope_pattern(raw: &str) -> Option<String> {
    let normalized = raw.trim().replace('\\', "/");
    let trimmed = normalized.trim_start_matches("./").trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn looks_like_path_pattern(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }

    let normalized = trimmed.replace('\\', "/");
    normalized == "*"
        || normalized == "**"
        || normalized.starts_with('.')
        || normalized.contains('/')
        || normalized.contains('*')
        || normalized.contains('?')
        || normalized.contains('[')
        || normalized.contains('{')
        || normalized
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::types::{
        ChangeType, Intent, Objective, ObjectiveKind, TouchHints,
    };

    fn intent() -> Intent {
        Intent {
            summary: "Improve quality".into(),
            problem_statement: "Tighten repo quality checks".into(),
            change_type: ChangeType::Chore,
            objectives: vec![Objective {
                title: "Fix issues".into(),
                kind: ObjectiveKind::Implementation,
            }],
            in_scope: vec![],
            out_of_scope: vec![],
            touch_hints: None,
        }
    }

    #[test]
    fn effective_write_scope_prefers_file_patterns_over_freeform_scope_text() {
        let mut intent = intent();
        intent.in_scope = vec![
            "Run and fix clippy warnings across the codebase".into(),
            "Fix error handling anti-patterns (unwrap/expect)".into(),
        ];
        intent.touch_hints = Some(TouchHints {
            files: vec!["src/**/*.rs".into(), "tests/**/*.rs".into()],
            symbols: vec![],
            apis: vec![],
        });

        assert_eq!(
            effective_write_scope(&intent),
            vec!["src/**/*.rs".to_string(), "tests/**/*.rs".to_string()]
        );
    }

    #[test]
    fn effective_write_scope_keeps_explicit_path_entries() {
        let mut intent = intent();
        intent.in_scope = vec![
            "src/".into(),
            "Cargo.toml".into(),
            "keep rustfmt aligned".into(),
        ];

        assert_eq!(
            effective_write_scope(&intent),
            vec!["Cargo.toml".to_string(), "src/".to_string()]
        );
    }

    #[test]
    fn effective_forbidden_scope_ignores_freeform_items() {
        let mut intent = intent();
        intent.out_of_scope = vec![
            "vendor/".into(),
            "Changing public API contracts".into(),
            ".github/workflows/**".into(),
        ];

        assert_eq!(
            effective_forbidden_scope(&intent),
            vec![".github/workflows/**".to_string(), "vendor/".to_string()]
        );
    }
}
