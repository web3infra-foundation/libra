//! CEX-09 dynamic prompt and intent tool-policy contract tests.

use std::{process::Command, sync::Arc};

use libra::internal::ai::{
    agent::TaskIntent,
    prompt::SystemPromptBuilder,
    tools::{
        ToolRegistryBuilder,
        handlers::{
            ApplyPatchHandler, ListDirHandler, ReadFileHandler, ShellHandler,
            register_semantic_handlers,
        },
    },
};
use tempfile::TempDir;

#[test]
fn ai_dynamic_prompt_includes_intent_budget_sources_trust_and_status() {
    let repo = TempDir::new().expect("temp repo");
    run_git(repo.path(), &["init"]);
    std::fs::write(repo.path().join("dirty.txt"), "untracked").expect("dirty file");

    let prompt = SystemPromptBuilder::new(repo.path())
        .with_intent(TaskIntent::Question)
        .with_dynamic_context()
        .build();

    assert!(prompt.contains("## Task Intent"));
    assert!(prompt.contains("intent=question"));
    assert!(prompt.contains("## Dynamic Workspace Context"));
    assert!(prompt.contains("source=libra status --short"));
    assert!(prompt.contains("dirty.txt"));
    assert!(prompt.contains("trust=trusted"));
    assert!(prompt.contains("trust=untrusted"));
    assert!(prompt.contains("## Context Budget Plan"));
    assert!(prompt.contains("system_rules"));
    assert!(prompt.contains("semantic_snippets"));
}

#[test]
fn ai_dynamic_prompt_reuses_rules_snapshot_within_ttl() {
    let repo = TempDir::new().expect("temp repo");
    let rules_dir = repo.path().join(".libra").join("rules");
    std::fs::create_dir_all(&rules_dir).expect("rules dir");
    std::fs::write(rules_dir.join("team.md"), "initial team rule").expect("initial rule");

    let first = SystemPromptBuilder::new(repo.path())
        .with_intent(TaskIntent::Feature)
        .with_dynamic_context()
        .build();
    assert!(first.contains("initial team rule"));

    std::fs::write(rules_dir.join("team.md"), "updated team rule").expect("updated rule");

    let second = SystemPromptBuilder::new(repo.path())
        .with_intent(TaskIntent::Feature)
        .with_dynamic_context()
        .build();
    assert!(second.contains("initial team rule"));
    assert!(!second.contains("updated team rule"));
}

#[test]
fn ai_dynamic_prompt_tool_policy_filters_mutating_tools_for_read_only_intents() {
    let repo = TempDir::new().expect("temp repo");
    let registry = register_semantic_handlers(
        ToolRegistryBuilder::with_working_dir(repo.path().to_path_buf())
            .register("read_file", Arc::new(ReadFileHandler))
            .register("list_dir", Arc::new(ListDirHandler))
            .register("apply_patch", Arc::new(ApplyPatchHandler))
            .register("shell", Arc::new(ShellHandler)),
    )
    .build();

    let question_tools = registry.filter_by_intent(TaskIntent::Question);
    assert!(question_tools.contains(&"read_file".to_string()));
    assert!(question_tools.contains(&"list_symbols".to_string()));
    assert!(!question_tools.contains(&"apply_patch".to_string()));
    assert!(!question_tools.contains(&"shell".to_string()));

    let review_tools = registry.filter_by_intent(TaskIntent::Review);
    assert!(review_tools.contains(&"read_file".to_string()));
    assert!(review_tools.contains(&"trace_callers".to_string()));
    assert!(!review_tools.contains(&"apply_patch".to_string()));
    assert!(!review_tools.contains(&"shell".to_string()));

    let command_tools = registry.filter_by_intent(TaskIntent::Command);
    assert!(command_tools.contains(&"shell".to_string()));
    assert!(!command_tools.contains(&"apply_patch".to_string()));

    let feature_tools = registry.filter_by_intent(TaskIntent::Feature);
    assert!(feature_tools.contains(&"apply_patch".to_string()));
    assert!(feature_tools.contains(&"shell".to_string()));
}

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("git command should run");
    assert!(status.success(), "git {:?} failed with {status}", args);
}
