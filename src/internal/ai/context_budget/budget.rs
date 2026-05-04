//! Context budget profiles and provider capability adaptation.

use std::{collections::HashSet, fmt};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_PROVIDER: &str = "runtime";
const DEFAULT_MODEL: &str = "default";

/// The seven context segments in the CEX-13a budget contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSegmentKind {
    SystemRules,
    ProjectMemory,
    MemoryAnchor,
    RecentMessages,
    ToolResults,
    SemanticSnippets,
    SourceContext,
}

impl ContextSegmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemRules => "system_rules",
            Self::ProjectMemory => "project_memory",
            Self::MemoryAnchor => "memory_anchor",
            Self::RecentMessages => "recent_messages",
            Self::ToolResults => "tool_results",
            Self::SemanticSnippets => "semantic_snippets",
            Self::SourceContext => "source_context",
        }
    }

    pub fn default_priority(self) -> ContextPriority {
        match self {
            Self::SystemRules => ContextPriority::Critical,
            Self::ProjectMemory | Self::MemoryAnchor | Self::RecentMessages => {
                ContextPriority::High
            }
            Self::ToolResults | Self::SemanticSnippets => ContextPriority::Medium,
            Self::SourceContext => ContextPriority::Low,
        }
    }
}

impl fmt::Display for ContextSegmentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Retention priority used when context must be trimmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPriority {
    Critical,
    High,
    Medium,
    Low,
}

impl ContextPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    pub(crate) fn retention_rank(self) -> u8 {
        match self {
            Self::Critical => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
        }
    }
}

impl fmt::Display for ContextPriority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a segment may be reduced when it exceeds its budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationPolicy {
    Never,
    OldestFirst,
    SummaryFirst,
    CompressLargeOutputs,
    DropLowConfidence,
    PreserveSourceLabels,
}

impl TruncationPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OldestFirst => "oldest_first",
            Self::SummaryFirst => "summary_first",
            Self::CompressLargeOutputs => "compress_large_outputs",
            Self::DropLowConfidence => "drop_low_confidence",
            Self::PreserveSourceLabels => "preserve_source_labels",
        }
    }
}

impl fmt::Display for TruncationPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Budget for one segment of context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSegmentBudget {
    pub kind: ContextSegmentKind,
    pub max_tokens: u64,
    pub priority: ContextPriority,
    pub truncation: TruncationPolicy,
    #[serde(default)]
    pub non_compressible: bool,
}

impl ContextSegmentBudget {
    pub fn new(kind: ContextSegmentKind, max_tokens: u64, truncation: TruncationPolicy) -> Self {
        Self {
            kind,
            max_tokens,
            priority: kind.default_priority(),
            truncation,
            non_compressible: truncation == TruncationPolicy::Never,
        }
    }

    pub fn priority(mut self, priority: ContextPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn non_compressible(mut self, value: bool) -> Self {
        self.non_compressible = value;
        self
    }
}

/// Provider context-window capability used to derive a prompt budget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderContextCapability {
    pub provider: String,
    pub model: String,
    pub max_context_tokens: u64,
    pub reserved_output_tokens: u64,
}

impl ProviderContextCapability {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        max_context_tokens: u64,
        reserved_output_tokens: u64,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            max_context_tokens,
            reserved_output_tokens,
        }
    }

    pub fn for_provider_model(provider: &str, model: &str) -> Self {
        let provider_lower = provider.to_ascii_lowercase();
        let model_lower = model.to_ascii_lowercase();
        let (max_context_tokens, reserved_output_tokens) = match provider_lower.as_str() {
            "ollama" => (local_model_context_tokens(&model_lower), 2_000),
            "anthropic" | "gemini" | "openai" | "codex" => (128_000, 16_000),
            "deepseek" | "kimi" | "zhipu" => (64_000, 8_000),
            "fake" => (16_000, 2_800),
            _ => (16_000, 2_800),
        };
        Self::new(
            provider_lower,
            if model.is_empty() {
                DEFAULT_MODEL.to_string()
            } else {
                model.to_string()
            },
            max_context_tokens,
            reserved_output_tokens,
        )
    }

    pub fn prompt_token_budget(&self) -> u64 {
        self.max_context_tokens
            .saturating_sub(self.reserved_output_tokens)
    }
}

fn local_model_context_tokens(model: &str) -> u64 {
    if model.contains("128k") {
        128_000
    } else if model.contains("64k") {
        64_000
    } else if model.contains("32k") {
        32_000
    } else if model.contains("16k") {
        16_000
    } else {
        8_000
    }
}

/// Complete budget profile for one provider/model prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBudget {
    max_prompt_tokens: u64,
    provider: ProviderContextCapability,
    segments: Vec<ContextSegmentBudget>,
}

impl ContextBudget {
    pub fn for_provider_model(provider: &str, model: &str) -> Self {
        let capability = ProviderContextCapability::for_provider_model(provider, model);
        Self::for_provider_capability(&capability)
    }

    pub fn for_provider_capability(capability: &ProviderContextCapability) -> Self {
        let prompt_budget = capability.prompt_token_budget();
        let base_segments = default_segments();
        let base_total = sum_segment_tokens(&base_segments);

        let segments = if prompt_budget >= base_total {
            base_segments
        } else {
            scale_segments_for_prompt_budget(&base_segments, prompt_budget)
        };

        Self {
            max_prompt_tokens: prompt_budget,
            provider: capability.clone(),
            segments,
        }
    }

    pub fn from_segments(
        max_prompt_tokens: u64,
        segments: Vec<ContextSegmentBudget>,
    ) -> Result<Self, ContextBudgetError> {
        validate_segments(&segments)?;
        Ok(Self {
            max_prompt_tokens,
            provider: ProviderContextCapability::new(
                DEFAULT_PROVIDER,
                DEFAULT_MODEL,
                max_prompt_tokens,
                0,
            ),
            segments,
        })
    }

    pub fn max_prompt_tokens(&self) -> u64 {
        self.max_prompt_tokens
    }

    pub fn provider(&self) -> &ProviderContextCapability {
        &self.provider
    }

    pub fn segments(&self) -> &[ContextSegmentBudget] {
        &self.segments
    }

    pub fn segment(&self, kind: ContextSegmentKind) -> Option<&ContextSegmentBudget> {
        self.segments.iter().find(|segment| segment.kind == kind)
    }

    pub fn total_segment_tokens(&self) -> u64 {
        sum_segment_tokens(&self.segments)
    }

    pub fn render_plan_section(&self) -> String {
        let mut lines = vec![
            "## Context Budget Plan".to_string(),
            String::new(),
            "source=runtime trust=trusted budget_tokens_max=260".to_string(),
            format!(
                "provider={} model={} max_context_tokens={} reserved_output_tokens={} max_prompt_tokens={}",
                self.provider.provider,
                self.provider.model,
                self.provider.max_context_tokens,
                self.provider.reserved_output_tokens,
                self.max_prompt_tokens,
            ),
        ];

        for segment in &self.segments {
            lines.push(format!(
                "- {}: max_tokens={} priority={} truncation={} non_compressible={}",
                segment.kind,
                segment.max_tokens,
                segment.priority,
                segment.truncation,
                segment.non_compressible
            ));
        }

        lines.join("\n")
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self::for_provider_capability(&ProviderContextCapability::new(
            DEFAULT_PROVIDER,
            DEFAULT_MODEL,
            16_000,
            2_800,
        ))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContextBudgetError {
    #[error("context budget must define at least one segment")]
    EmptySegments,
    #[error("context budget contains duplicate segment '{0}'")]
    DuplicateSegment(ContextSegmentKind),
}

fn validate_segments(segments: &[ContextSegmentBudget]) -> Result<(), ContextBudgetError> {
    if segments.is_empty() {
        return Err(ContextBudgetError::EmptySegments);
    }

    let mut seen = HashSet::new();
    for segment in segments {
        if !seen.insert(segment.kind) {
            return Err(ContextBudgetError::DuplicateSegment(segment.kind));
        }
    }

    Ok(())
}

fn default_segments() -> Vec<ContextSegmentBudget> {
    vec![
        ContextSegmentBudget::new(
            ContextSegmentKind::SystemRules,
            3_200,
            TruncationPolicy::Never,
        )
        .priority(ContextPriority::Critical)
        .non_compressible(true),
        ContextSegmentBudget::new(
            ContextSegmentKind::ProjectMemory,
            1_600,
            TruncationPolicy::OldestFirst,
        ),
        ContextSegmentBudget::new(
            ContextSegmentKind::MemoryAnchor,
            1_200,
            TruncationPolicy::OldestFirst,
        ),
        ContextSegmentBudget::new(
            ContextSegmentKind::RecentMessages,
            2_400,
            TruncationPolicy::SummaryFirst,
        ),
        ContextSegmentBudget::new(
            ContextSegmentKind::ToolResults,
            1_800,
            TruncationPolicy::CompressLargeOutputs,
        ),
        ContextSegmentBudget::new(
            ContextSegmentKind::SemanticSnippets,
            1_400,
            TruncationPolicy::DropLowConfidence,
        ),
        ContextSegmentBudget::new(
            ContextSegmentKind::SourceContext,
            1_600,
            TruncationPolicy::PreserveSourceLabels,
        ),
    ]
}

fn scale_segments_for_prompt_budget(
    base_segments: &[ContextSegmentBudget],
    prompt_budget: u64,
) -> Vec<ContextSegmentBudget> {
    let fixed_tokens: u64 = base_segments
        .iter()
        .filter(|segment| segment.non_compressible)
        .map(|segment| segment.max_tokens)
        .sum();
    let remaining_tokens = prompt_budget.saturating_sub(fixed_tokens);
    let compressible_base: u64 = base_segments
        .iter()
        .filter(|segment| !segment.non_compressible)
        .map(|segment| segment.max_tokens)
        .sum();

    if compressible_base == 0 {
        return base_segments.to_vec();
    }

    let mut scaled = Vec::with_capacity(base_segments.len());
    let mut allocated_compressible = 0_u64;
    for segment in base_segments {
        let mut segment = segment.clone();
        if !segment.non_compressible {
            segment.max_tokens =
                segment.max_tokens.saturating_mul(remaining_tokens) / compressible_base;
            allocated_compressible = allocated_compressible.saturating_add(segment.max_tokens);
        }
        scaled.push(segment);
    }

    let mut leftover = remaining_tokens.saturating_sub(allocated_compressible);
    while leftover > 0 {
        let Some(segment) = scaled
            .iter_mut()
            .filter(|segment| !segment.non_compressible)
            .min_by_key(|segment| segment.priority.retention_rank())
        else {
            break;
        };
        segment.max_tokens = segment.max_tokens.saturating_add(1);
        leftover -= 1;
    }

    scaled
}

fn sum_segment_tokens(segments: &[ContextSegmentBudget]) -> u64 {
    segments
        .iter()
        .map(|segment| segment.max_tokens)
        .fold(0_u64, u64::saturating_add)
}
