//! Prompt context-frame records and attachment handling.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    AllocationOmissionReason, ContextAllocationOmission, ContextBudget, ContextBudgetAllocator,
    ContextBudgetCandidate, ContextSegmentKind,
};
use crate::internal::ai::runtime::event::Event;

const DEFAULT_ATTACHMENT_THRESHOLD_BYTES: usize = 8 * 1024;
const ATTACHMENTS_DIR: &str = "attachments";

/// Coarse context-frame kind for session JSONL replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextFrameKind {
    PromptBuild,
    ToolResult,
    ResumeAudit,
    CompactionSummary,
}

impl ContextFrameKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PromptBuild => "prompt_build",
            Self::ToolResult => "tool_result",
            Self::ResumeAudit => "resume_audit",
            Self::CompactionSummary => "compaction_summary",
        }
    }
}

/// Trust tier for a context segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextTrustLevel {
    Trusted,
    Untrusted,
    External,
}

/// Source label for a context segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextFrameSource {
    pub kind: ContextFrameSourceKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ContextFrameSource {
    pub fn runtime(label: impl Into<String>) -> Self {
        Self::new(ContextFrameSourceKind::Runtime, label, None)
    }

    pub fn file(path: impl Into<String>) -> Self {
        Self::new(ContextFrameSourceKind::File, path, None)
    }

    pub fn tool(tool_name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(ContextFrameSourceKind::Tool, tool_name, Some(detail.into()))
    }

    pub fn mcp(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(ContextFrameSourceKind::Mcp, label, Some(detail.into()))
    }

    pub fn hook(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(ContextFrameSourceKind::Hook, label, Some(detail.into()))
    }

    pub fn web(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(ContextFrameSourceKind::Web, label, Some(detail.into()))
    }

    fn new(kind: ContextFrameSourceKind, label: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            detail,
        }
    }
}

/// Source category for a context segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextFrameSourceKind {
    Runtime,
    File,
    Tool,
    Mcp,
    Hook,
    Web,
}

/// Reference to large context content stored outside the main JSONL event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextAttachmentRef {
    pub sha256: String,
    pub bytes: u64,
    pub line_count: usize,
    pub relative_path: String,
    pub read_hint: String,
}

/// One included prompt context segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextFrameSegment {
    pub id: String,
    pub segment: ContextSegmentKind,
    pub source: ContextFrameSource,
    pub trust: ContextTrustLevel,
    pub token_estimate: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<ContextAttachmentRef>,
    #[serde(default)]
    pub non_compressible: bool,
}

/// Append-only prompt context frame event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextFrameEvent {
    pub event_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub frame_id: Uuid,
    pub kind: ContextFrameKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    pub segments: Vec<ContextFrameSegment>,
    pub omissions: Vec<ContextFrameOmission>,
    pub total_candidate_tokens: u64,
    pub total_selected_tokens: u64,
    #[serde(default)]
    pub budget_exceeded_by: u64,
}

impl ContextFrameEvent {
    pub fn attachment_refs(&self) -> Vec<ContextAttachmentRef> {
        self.segments
            .iter()
            .filter_map(|segment| segment.attachment.clone())
            .collect()
    }

    /// Materialise this frame's segments into a `Vec<Message>` the
    /// child sub-agent's tool loop can consume as
    /// pre-populated history before its own user prompt lands.
    ///
    /// This is OC-Phase 4 P4.4's "minimum-viable handoff" — the
    /// compaction-agent SUMMARY step lands as a follow-up that
    /// replaces the raw segment dump with a parser-validated
    /// 8-section template via `ContextHandoffBuilder` (v0.17.740).
    /// Until that lands, the child sees the parent's working
    /// context as User messages, each prefixed with a
    /// `[<segment_kind>:<source_kind>:<source_label>]` header so
    /// the model can attribute the content.
    ///
    /// Filter rule: only segments with `content = Some` are
    /// surfaced. Attachment-only segments (where the content was
    /// externalised to a sidecar file) are skipped because the
    /// child cannot meaningfully act on a path it has not yet
    /// read — the `attachment_refs` array on the handoff carries
    /// the references separately, and a future tool call from
    /// the child resolves them. Empty-content segments produce no
    /// message.
    pub fn to_handoff_messages(&self) -> Vec<crate::internal::ai::completion::Message> {
        use crate::internal::ai::completion::Message;
        self.segments
            .iter()
            .filter_map(|segment| {
                let content = segment.content.as_deref()?;
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let header = format!(
                    "[{}:{:?}:{}]",
                    segment.segment.as_str(),
                    segment.source.kind,
                    segment.source.label,
                );
                Some(Message::user(format!("{header}\n{trimmed}")))
            })
            .collect()
    }
}

impl Event for ContextFrameEvent {
    fn event_kind(&self) -> &'static str {
        "context_frame"
    }

    fn event_id(&self) -> Uuid {
        self.event_id
    }

    fn event_summary(&self) -> String {
        format!(
            "{} context frame {} with {} segment(s)",
            self.kind.as_str(),
            self.frame_id,
            self.segments.len()
        )
    }
}

/// Omitted prompt context segment with deterministic truncation reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextFrameOmission {
    pub id: String,
    pub segment: ContextSegmentKind,
    pub token_estimate: u64,
    pub reason: AllocationOmissionReason,
}

impl From<ContextAllocationOmission> for ContextFrameOmission {
    fn from(value: ContextAllocationOmission) -> Self {
        Self {
            id: value.id,
            segment: value.segment,
            token_estimate: value.token_estimate,
            reason: value.reason,
        }
    }
}

/// Candidate context segment before budget allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextFrameCandidate {
    id: String,
    segment: ContextSegmentKind,
    content: String,
    source: ContextFrameSource,
    trust: ContextTrustLevel,
    token_estimate: Option<u64>,
    non_compressible: bool,
}

impl ContextFrameCandidate {
    pub fn new(
        id: impl Into<String>,
        segment: ContextSegmentKind,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            segment,
            content: content.into(),
            source: ContextFrameSource::runtime("unknown"),
            trust: ContextTrustLevel::Untrusted,
            token_estimate: None,
            non_compressible: false,
        }
    }

    pub fn source(mut self, source: ContextFrameSource) -> Self {
        self.source = source;
        self
    }

    pub fn trust(mut self, trust: ContextTrustLevel) -> Self {
        self.trust = trust;
        self
    }

    pub fn token_estimate(mut self, token_estimate: u64) -> Self {
        self.token_estimate = Some(token_estimate);
        self
    }

    pub fn non_compressible(mut self, value: bool) -> Self {
        self.non_compressible = value;
        self
    }

    fn resolved_token_estimate(&self) -> u64 {
        self.token_estimate
            .unwrap_or_else(|| estimate_tokens(&self.content))
    }
}

/// Builds a context frame by applying the provider-aware budget allocator.
#[derive(Clone, Debug)]
pub struct ContextFrameBuilder {
    kind: ContextFrameKind,
    budget: ContextBudget,
    prompt_id: Option<String>,
    attachment_threshold_bytes: usize,
    candidates: Vec<ContextFrameCandidate>,
}

impl ContextFrameBuilder {
    pub fn new(kind: ContextFrameKind, budget: ContextBudget) -> Self {
        Self {
            kind,
            budget,
            prompt_id: None,
            attachment_threshold_bytes: DEFAULT_ATTACHMENT_THRESHOLD_BYTES,
            candidates: Vec::new(),
        }
    }

    pub fn with_prompt_id(mut self, prompt_id: impl Into<String>) -> Self {
        self.prompt_id = Some(prompt_id.into());
        self
    }

    pub fn with_attachment_threshold_bytes(mut self, bytes: usize) -> Self {
        self.attachment_threshold_bytes = bytes;
        self
    }

    pub fn push(mut self, candidate: ContextFrameCandidate) -> Self {
        self.candidates.push(candidate);
        self
    }

    pub fn build(self, attachments: &ContextAttachmentStore) -> io::Result<ContextFrameEvent> {
        let total_candidate_tokens = self.candidates.iter().fold(0_u64, |total, candidate| {
            total.saturating_add(candidate.resolved_token_estimate())
        });

        let allocation_candidates = self
            .candidates
            .iter()
            .map(|candidate| {
                ContextBudgetCandidate::new(
                    candidate.id.clone(),
                    candidate.segment,
                    candidate.resolved_token_estimate(),
                )
                .non_compressible(candidate.non_compressible)
            })
            .collect();

        let allocation =
            ContextBudgetAllocator::new(self.budget.clone()).allocate(allocation_candidates);
        let selected_ids: HashSet<&str> = allocation
            .selected()
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect();

        let mut segments = Vec::new();
        for candidate in self
            .candidates
            .into_iter()
            .filter(|candidate| selected_ids.contains(candidate.id.as_str()))
        {
            let token_estimate = candidate.resolved_token_estimate();
            let non_compressible = candidate.non_compressible
                || self
                    .budget
                    .segment(candidate.segment)
                    .is_some_and(|segment| segment.non_compressible);
            let should_attach =
                !non_compressible && candidate.content.len() > self.attachment_threshold_bytes;
            let (content, summary, attachment) = if should_attach {
                let attachment = attachments.write_content(&candidate.content)?;
                (
                    None,
                    Some(summarize_content(&candidate.content)),
                    Some(attachment),
                )
            } else {
                (Some(candidate.content.clone()), None, None)
            };

            segments.push(ContextFrameSegment {
                id: candidate.id,
                segment: candidate.segment,
                source: candidate.source,
                trust: candidate.trust,
                token_estimate,
                content,
                summary,
                attachment,
                non_compressible,
            });
        }

        Ok(ContextFrameEvent {
            event_id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            frame_id: Uuid::new_v4(),
            kind: self.kind,
            prompt_id: self.prompt_id,
            segments,
            omissions: allocation
                .omitted()
                .iter()
                .cloned()
                .map(ContextFrameOmission::from)
                .collect(),
            total_candidate_tokens,
            total_selected_tokens: allocation.total_selected_tokens(),
            budget_exceeded_by: allocation.budget_exceeded_by(),
        })
    }
}

/// Filesystem store for large context-frame attachments.
#[derive(Clone, Debug)]
pub struct ContextAttachmentStore {
    session_root: PathBuf,
}

impl ContextAttachmentStore {
    pub fn new(session_root: impl AsRef<Path>) -> Self {
        Self {
            session_root: session_root.as_ref().to_path_buf(),
        }
    }

    pub fn write_content(&self, content: &str) -> io::Result<ContextAttachmentRef> {
        let hash = sha256_hex(content.as_bytes());
        let relative_path = format!("{ATTACHMENTS_DIR}/{hash}");
        let path = self.session_root.join(&relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!(
                        "failed to create context attachment directory '{}': {err}",
                        parent.display()
                    ),
                )
            })?;
        }
        fs::write(&path, content).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to write context attachment '{}': {err}",
                    path.display()
                ),
            )
        })?;

        Ok(ContextAttachmentRef {
            sha256: hash,
            bytes: content.len() as u64,
            line_count: count_lines(content),
            relative_path: relative_path.clone(),
            read_hint: format!("read .libra session attachment at {relative_path}"),
        })
    }

    pub fn read_to_string(&self, attachment: &ContextAttachmentRef) -> io::Result<String> {
        let path = self.session_root.join(&attachment.relative_path);
        fs::read_to_string(&path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to read context attachment '{}': {err}",
                    path.display()
                ),
            )
        })
    }
}

fn estimate_tokens(content: &str) -> u64 {
    let chars = content.chars().count() as u64;
    chars.saturating_add(3).saturating_div(4).max(1)
}

fn summarize_content(content: &str) -> String {
    let first_line = content.lines().next().unwrap_or_default();
    let mut summary: String = first_line.chars().take(120).collect();
    if first_line.chars().count() > 120 {
        summary.push_str("...");
    }
    summary
}

fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(digest(&SHA256, bytes).as_ref())
}

#[cfg(test)]
mod handoff_message_tests {
    use super::*;
    use crate::internal::ai::completion::{Message, UserContent};

    fn segment(
        id: &str,
        kind: ContextSegmentKind,
        source_kind: ContextFrameSourceKind,
        label: &str,
        content: Option<&str>,
    ) -> ContextFrameSegment {
        ContextFrameSegment {
            id: id.to_string(),
            segment: kind,
            source: ContextFrameSource {
                kind: source_kind,
                label: label.to_string(),
                detail: None,
            },
            trust: ContextTrustLevel::Trusted,
            token_estimate: 0,
            content: content.map(|s| s.to_string()),
            summary: None,
            attachment: None,
            non_compressible: false,
        }
    }

    fn frame_with_segments(segments: Vec<ContextFrameSegment>) -> ContextFrameEvent {
        ContextFrameEvent {
            event_id: Uuid::new_v4(),
            recorded_at: Utc::now(),
            frame_id: Uuid::new_v4(),
            kind: ContextFrameKind::ToolResult,
            prompt_id: None,
            segments,
            omissions: Vec::new(),
            total_candidate_tokens: 0,
            total_selected_tokens: 0,
            budget_exceeded_by: 0,
        }
    }

    /// Each segment with non-empty content becomes one User message
    /// whose body starts with a `[kind:source:label]` header line.
    /// The header lets the model attribute the content while
    /// reading the handoff.
    #[test]
    fn to_handoff_messages_wraps_each_content_segment_with_header() {
        let frame = frame_with_segments(vec![
            segment(
                "seg-1",
                ContextSegmentKind::RecentMessages,
                ContextFrameSourceKind::Runtime,
                "transcript",
                Some("hello from the parent"),
            ),
            segment(
                "seg-2",
                ContextSegmentKind::ToolResults,
                ContextFrameSourceKind::Tool,
                "shell",
                Some("$ ls\nfile.txt"),
            ),
        ]);

        let messages = frame.to_handoff_messages();
        assert_eq!(messages.len(), 2);

        let texts: Vec<String> = messages
            .iter()
            .map(|m| match m {
                Message::User { content } => content
                    .iter()
                    .filter_map(|c| match c {
                        UserContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
                _ => panic!("handoff must produce User messages, got: {m:?}"),
            })
            .collect();

        assert!(
            texts[0].starts_with("[recent_messages:Runtime:transcript]\n"),
            "first message must carry the segment header, got: {}",
            texts[0],
        );
        assert!(
            texts[0].contains("hello from the parent"),
            "first message must include the trimmed segment content, got: {}",
            texts[0],
        );
        assert!(
            texts[1].starts_with("[tool_results:Tool:shell]\n"),
            "second message header must reflect its segment kind/source, got: {}",
            texts[1],
        );
    }

    /// Segments with `content = None` (externalised to attachment)
    /// AND segments whose content is whitespace-only do NOT produce
    /// messages. The attachment_refs path carries the externalised
    /// data separately; an empty user message would just waste the
    /// child's token budget without adding signal.
    #[test]
    fn to_handoff_messages_skips_attachment_only_and_empty_content_segments() {
        let frame = frame_with_segments(vec![
            // Attachment-only: content = None
            segment(
                "seg-attach",
                ContextSegmentKind::ToolResults,
                ContextFrameSourceKind::Tool,
                "build",
                None,
            ),
            // Whitespace-only content
            segment(
                "seg-blank",
                ContextSegmentKind::RecentMessages,
                ContextFrameSourceKind::Runtime,
                "user",
                Some("   \n\t  "),
            ),
            // Real content survives
            segment(
                "seg-real",
                ContextSegmentKind::SystemRules,
                ContextFrameSourceKind::Runtime,
                "preamble",
                Some("system rule"),
            ),
        ]);

        let messages = frame.to_handoff_messages();
        assert_eq!(
            messages.len(),
            1,
            "only the real-content segment should produce a message; got {messages:?}",
        );
    }
}
