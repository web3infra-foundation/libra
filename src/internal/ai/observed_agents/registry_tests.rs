use super::*;

#[test]
fn agent_for_returns_matching_kind_for_every_variant() {
    for kind in AgentKind::all() {
        let agent = agent_for(*kind);
        assert_eq!(agent.provider_kind(), *kind);
        assert_eq!(agent.stability(), AgentStability::Stable);
    }
}

#[test]
fn agent_for_returns_stable_static_references_across_calls() {
    for kind in AgentKind::all() {
        let a = agent_for(*kind);
        let b = agent_for(*kind);
        assert!(
            std::ptr::eq(
                a as *const dyn ObservedAgent as *const (),
                b as *const dyn ObservedAgent as *const (),
            ),
            "agent_for({kind:?}) must return the same reference",
        );
    }
}

#[test]
fn truncator_for_returns_expected_optional_capabilities() {
    for kind in AgentKind::all() {
        let truncator = truncator_for(*kind);
        let should_have_truncator = matches!(
            *kind,
            AgentKind::ClaudeCode
                | AgentKind::Gemini
                | AgentKind::Cursor
                | AgentKind::Codex
                | AgentKind::OpenCode
                | AgentKind::Copilot
        );
        assert_eq!(
            truncator.is_some(),
            should_have_truncator,
            "truncator_for({kind:?}) optional-capability mismatch",
        );
    }
}

#[test]
fn truncator_for_some_arm_reports_matching_kind() {
    for kind in AgentKind::all() {
        if let Some(truncator) = truncator_for(*kind) {
            assert_eq!(truncator.provider_kind(), *kind);
        }
    }
}

#[test]
fn agent_for_protected_dirs_are_dot_prefixed_and_non_empty() {
    for kind in AgentKind::all() {
        let agent = agent_for(*kind);
        let dirs = agent.protected_dirs();
        assert!(
            !dirs.is_empty(),
            "agent_for({kind:?}) returned empty protected_dirs",
        );
        for dir in dirs {
            assert!(
                dir.starts_with('.'),
                "agent_for({kind:?}) protected_dir '{dir}' must start with '.'",
            );
        }
    }
}
