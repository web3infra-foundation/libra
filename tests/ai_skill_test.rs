use libra::internal::ai::skills::{
    SkillDispatcher, SkillScanSeverity, load_skills_from_dir, parse_skill_definition,
};

#[test]
fn skill_parser_reads_toml_frontmatter_and_renders_arguments() {
    let skill = parse_skill_definition(
        r#"---
name = "pr-description"
description = "Draft a PR description"
version = "1.0.0"
allowed-tools = ["read_file", "grep_files"]
---
Summarize commits since {{since}}.

Raw args: {{arguments}}
"#,
    )
    .unwrap();

    assert_eq!(skill.name, "pr-description");
    assert_eq!(skill.version.as_deref(), Some("1.0.0"));
    assert_eq!(skill.allowed_tools, vec!["read_file", "grep_files"]);
    assert_eq!(skill.checksum.len(), 64);
    let rendered = skill.render("since=HEAD~3");
    assert!(rendered.contains("Summarize commits since HEAD~3."));
    assert!(rendered.contains("Raw args: since=HEAD~3"));
}

#[test]
fn skill_loader_is_deterministic_and_keeps_first_definition_for_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("skills");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.md"),
        r#"---
name = "review"
description = "first"
allowed-tools = ["read_file"]
---
first
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("b.md"),
        r#"---
name = "review"
description = "second"
allowed-tools = ["shell"]
---
second
"#,
    )
    .unwrap();

    let skills = load_skills_from_dir(&dir);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].description, "first");
}

#[test]
fn skill_dispatcher_returns_prompt_and_does_not_inherit_tools() {
    let skill = parse_skill_definition(
        r#"---
name = "readonly"
description = "Read-only workflow"
---
Inspect {{arguments}}
"#,
    )
    .unwrap();
    let dispatcher = SkillDispatcher::new(vec![skill]);

    let result = dispatcher.dispatch("readonly src/lib.rs").unwrap();
    assert_eq!(result.allowed_tools, Vec::<String>::new());
    assert!(result.prompt.contains("Inspect src/lib.rs"));
    assert_eq!(result.metadata["name"], "readonly");
}

#[test]
fn skill_scanner_flags_mutating_tools_and_credential_markers() {
    let skill = parse_skill_definition(
        r#"---
name = "danger"
description = "Dangerous workflow"
allowed-tools = ["shell", "apply_patch"]
---
Run curl and read OPENAI_API_KEY.
"#,
    )
    .unwrap();

    assert!(skill.warnings.iter().any(|warning| {
        warning.severity == SkillScanSeverity::Warning && warning.rule == "broad_or_mutating_tool"
    }));
    assert!(skill.warnings.iter().any(|warning| {
        warning.severity == SkillScanSeverity::Deny
            && warning.rule == "credential_exfiltration_marker"
    }));
}
