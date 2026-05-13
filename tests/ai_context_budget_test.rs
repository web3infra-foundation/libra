//! CEX-13a context budget core contract tests.

use libra::internal::ai::{
    context_budget::{
        AllocationOmissionReason, ContextBudget, ContextBudgetAllocator, ContextBudgetCandidate,
        ContextSegmentBudget, ContextSegmentKind, ProviderContextCapability, TruncationPolicy,
    },
    prompt::SystemPromptBuilder,
};
use tempfile::TempDir;

#[test]
fn context_budget_default_profile_has_seven_segments_and_noncompressible_rules() {
    let budget = ContextBudget::default();
    let kinds: Vec<_> = budget
        .segments()
        .iter()
        .map(|segment| segment.kind)
        .collect();

    assert_eq!(
        kinds,
        vec![
            ContextSegmentKind::SystemRules,
            ContextSegmentKind::ProjectMemory,
            ContextSegmentKind::MemoryAnchor,
            ContextSegmentKind::RecentMessages,
            ContextSegmentKind::ToolResults,
            ContextSegmentKind::SemanticSnippets,
            ContextSegmentKind::SourceContext,
        ]
    );

    let system_rules = budget
        .segment(ContextSegmentKind::SystemRules)
        .expect("system rules segment");
    assert_eq!(system_rules.truncation, TruncationPolicy::Never);
    assert!(system_rules.non_compressible);
}

#[test]
fn provider_capability_scales_compressible_segments_but_preserves_system_rules() {
    let compact = ProviderContextCapability::new("ollama", "small-local-model", 8_000, 2_000);
    let compact_budget = ContextBudget::for_provider_capability(&compact);
    let default_budget = ContextBudget::default();

    assert_eq!(compact_budget.max_prompt_tokens(), 6_000);
    assert_eq!(
        compact_budget
            .segment(ContextSegmentKind::SystemRules)
            .expect("system rules")
            .max_tokens,
        default_budget
            .segment(ContextSegmentKind::SystemRules)
            .expect("default system rules")
            .max_tokens,
        "safety rules must not shrink when provider context is small"
    );

    assert!(
        compact_budget
            .segment(ContextSegmentKind::ToolResults)
            .expect("tool results")
            .max_tokens
            < default_budget
                .segment(ContextSegmentKind::ToolResults)
                .expect("default tool results")
                .max_tokens
    );
    assert!(compact_budget.total_segment_tokens() <= compact_budget.max_prompt_tokens());
}

#[test]
fn allocator_drops_low_priority_context_before_high_priority_context() {
    let budget = ContextBudget::from_segments(
        500,
        vec![
            ContextSegmentBudget::new(
                ContextSegmentKind::SystemRules,
                200,
                TruncationPolicy::Never,
            )
            .non_compressible(true),
            ContextSegmentBudget::new(
                ContextSegmentKind::ProjectMemory,
                150,
                TruncationPolicy::OldestFirst,
            ),
            ContextSegmentBudget::new(
                ContextSegmentKind::RecentMessages,
                180,
                TruncationPolicy::SummaryFirst,
            ),
            ContextSegmentBudget::new(
                ContextSegmentKind::SourceContext,
                350,
                TruncationPolicy::PreserveSourceLabels,
            ),
        ],
    )
    .expect("budget");

    let allocation = ContextBudgetAllocator::new(budget).allocate(vec![
        ContextBudgetCandidate::new("system", ContextSegmentKind::SystemRules, 180),
        ContextBudgetCandidate::new("source", ContextSegmentKind::SourceContext, 300),
        ContextBudgetCandidate::new("memory", ContextSegmentKind::ProjectMemory, 140),
        ContextBudgetCandidate::new("recent", ContextSegmentKind::RecentMessages, 150),
    ]);

    assert!(allocation.selected_ids().contains(&"system"));
    assert!(allocation.selected_ids().contains(&"memory"));
    assert!(allocation.selected_ids().contains(&"recent"));
    assert!(!allocation.selected_ids().contains(&"source"));
    assert_eq!(
        allocation
            .omission_for("source")
            .expect("source omitted")
            .reason,
        AllocationOmissionReason::TotalBudgetExceeded
    );
}

#[test]
fn allocator_keeps_noncompressible_safety_rules_even_when_they_exceed_budget() {
    let budget = ContextBudget::from_segments(
        100,
        vec![
            ContextSegmentBudget::new(
                ContextSegmentKind::SystemRules,
                100,
                TruncationPolicy::Never,
            )
            .non_compressible(true),
            ContextSegmentBudget::new(
                ContextSegmentKind::SourceContext,
                100,
                TruncationPolicy::PreserveSourceLabels,
            ),
        ],
    )
    .expect("budget");

    let allocation = ContextBudgetAllocator::new(budget).allocate(vec![
        ContextBudgetCandidate::new("security", ContextSegmentKind::SystemRules, 180)
            .non_compressible(true),
        ContextBudgetCandidate::new("source", ContextSegmentKind::SourceContext, 10),
    ]);

    assert_eq!(allocation.selected_ids(), vec!["security"]);
    assert_eq!(allocation.budget_exceeded_by(), 80);
    assert_eq!(
        allocation
            .omission_for("source")
            .expect("source omitted")
            .reason,
        AllocationOmissionReason::TotalBudgetExceeded
    );
}

#[test]
fn system_prompt_renders_provider_adjusted_context_budget_plan() {
    let temp = TempDir::new().expect("temp dir");
    let budget = ContextBudget::for_provider_capability(&ProviderContextCapability::new(
        "ollama",
        "small-local-model",
        8_000,
        2_000,
    ));

    let prompt = SystemPromptBuilder::new(temp.path())
        .with_dynamic_context()
        .with_context_budget(budget)
        .build();

    assert!(prompt.contains("## Context Budget Plan"));
    assert!(prompt.contains("provider=ollama model=small-local-model"));
    assert!(prompt.contains("max_prompt_tokens=6000"));
    assert!(prompt.contains("- memory_anchor:"));
    assert!(prompt.contains("- source_context:"));
    assert!(!prompt.contains("dynamic_workspace_context"));
    assert!(!prompt.contains("untrusted_sources"));
}
