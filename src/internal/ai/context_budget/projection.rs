//! Compaction-aware transcript projection + tool-output prune.
//!
//! OC-Phase 4 P4.5 deliverable from `docs/improvement/opencode.md`.
//! This module owns two transformations that the dispatcher applies
//! to a chronological transcript before it ships to the model:
//!
//! 1. [`filter_compacted`] — opencode's PR #25851 message reorder
//!    (commit `a3eb…` on 2026-05-05). After a compaction event the
//!    sequence the model sees is **not** the chronological order
//!    `[..., tail, marker, summary, post]` but the rearranged
//!    `[marker, summary, tail, post]`. The model "first learns
//!    history was compacted", "then reads the summary", "then sees
//!    the retained tail", "then the new turns" — that ordering is
//!    materially friendlier to the model than chronological because
//!    it puts the high-information `summary` early.
//! 2. [`prune_inline_tool_output`] — replaces an oversized inline
//!    tool result with a `<pruned attachment_id="…" length="…">`
//!    placeholder so the model still sees the entry point but the
//!    inline tokens are recovered. Preserves attachment refs;
//!    refuses to touch tools listed in
//!    [`super::compaction::PRUNE_PROTECTED_TOOLS`]. Pure projection
//!    — does NOT mutate the source [`super::ContextFrameSegment`]
//!    or any persisted JSONL.
//!
//! What this module is **not**:
//! - It does not call the compaction agent (that is OC-Phase 4 P4.4
//!   in [`super::compaction_agent`]).
//! - It does not decide *when* to compact (the budget calculator
//!   surfaces the signal via `is_overflow` / `usable`).
//! - It does not mutate the [`super::ContextFrameEvent`] — the
//!   doc's "non-destructive inline replace" rule means the source
//!   bytes stay untouched and only the projection sent to the
//!   model is rewritten.

use super::compaction::{CompactionEvent, PRUNE_PROTECTED_TOOLS, TOOL_OUTPUT_MAX_CHARS};

/// Discriminant the projection algorithm needs to identify each
/// transcript entry. The dispatcher (P3 / P4) maps its richer
/// session message types into this lightweight shape so the reorder
/// rule stays a function on plain data — no implicit dependencies
/// on session storage or DB schema.
///
/// Field semantics:
/// - `User` / `Assistant` — ordinary chat turns.
/// - `Compaction { tail_start_id }` — a [`super::CompactionEvent`]
///   marker. `tail_start_id` is the id of the first message the
///   runtime kept as the post-compaction "retained tail" (mirrors
///   opencode `CompactionPart.tailStartID`). `None` means the
///   compaction kept no tail.
/// - `Summary { parent_compaction_id }` — an assistant message
///   whose body is the
///   [`super::handoff::parse_handoff_template`]-validated summary.
///   The `parent_compaction_id` links it to the marker that
///   triggered it; the reorder rule uses this link to decide which
///   `Compaction` to pair with which `Summary`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionKind {
    User,
    Assistant,
    Compaction { tail_start_id: Option<String> },
    Summary { parent_compaction_id: String },
}

/// Lightweight projection of a transcript message used as input to
/// [`filter_compacted`]. `id` must be stable across the session so
/// `Compaction.tail_start_id` and `Summary.parent_compaction_id`
/// can resolve to specific positions; the dispatcher typically uses
/// the JSONL row id or the segment id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageProjection {
    pub id: String,
    pub kind: ProjectionKind,
}

impl MessageProjection {
    /// Convenience constructor for callers that want to build a
    /// fixture without typing out the struct literal each time.
    pub fn new(id: impl Into<String>, kind: ProjectionKind) -> Self {
        Self {
            id: id.into(),
            kind,
        }
    }

    fn compaction_tail(&self) -> Option<&str> {
        match &self.kind {
            ProjectionKind::Compaction { tail_start_id } => tail_start_id.as_deref(),
            _ => None,
        }
    }

    fn summary_parent(&self) -> Option<&str> {
        match &self.kind {
            ProjectionKind::Summary {
                parent_compaction_id,
            } => Some(parent_compaction_id.as_str()),
            _ => None,
        }
    }
}

/// Lift a persisted [`CompactionEvent`] into the lightweight
/// projection input the reorder rule consumes. This defines the
/// canonical id space used by the dispatcher: a compaction's
/// `MessageProjection.id` is `event.event_id.to_string()`, and the
/// matching `Summary.parent_compaction_id` MUST use the same
/// stringified UUID so the search at
/// [`filter_compacted`] can pair the two by string equality.
///
/// The dispatcher (OC-Phase 3) uses this helper instead of
/// constructing the projection inline so the id-space contract
/// stays in one place and any future migration (e.g. to a
/// session-row-id space) lands here too.
pub fn compaction_event_to_projection(event: &CompactionEvent) -> MessageProjection {
    MessageProjection::new(
        event.event_id.to_string(),
        ProjectionKind::Compaction {
            tail_start_id: event.tail_start_id.clone(),
        },
    )
}

/// Apply the PR #25851 compaction reorder rule.
///
/// Algorithm (mirrors `session/message-v2.ts:1106-1133`):
///
/// 1. Walk the input chronologically and identify the **last**
///    `Compaction` whose `tail_start_id` is `Some` — call this the
///    **active compaction**. This is the latest compaction whose
///    tail boundary is well defined; older compactions are already
///    superseded by the latest summary.
/// 2. Find the `Summary` that comes after the active compaction
///    and whose `parent_compaction_id` matches it.
/// 3. Find the index of the message whose id equals the active
///    compaction's `tail_start_id`.
/// 4. If `tail_index < compaction_index < summary_index`, return
///    `[messages[compaction..=summary], messages[tail..compaction],
///    messages[summary+1..]]` — the rearranged ordering.
/// 5. Otherwise return the input slice in chronological order
///    unchanged. This is the case when:
///    - no compaction with `Some(tail_start_id)` exists in the
///      transcript (every compaction either has no tail, or there
///      are no compactions at all),
///    - the summary appears BEFORE the compaction (malformed
///      input, but we still serve a stable order),
///    - the summary parent does not match (defensive: pretend the
///      reorder isn't applicable),
///    - the `tail_start_id` does not resolve to any message in
///      the slice (defensive: same fallback).
///
/// The function returns borrows so the caller can splice without
/// cloning; the dispatcher renders to provider-native messages
/// downstream.
pub fn filter_compacted(messages: &[MessageProjection]) -> Vec<&MessageProjection> {
    let active_compaction_idx = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, m)| m.compaction_tail().is_some())
        .map(|(idx, _)| idx);

    let Some(comp_idx) = active_compaction_idx else {
        return messages.iter().collect();
    };

    let comp_id = messages[comp_idx].id.as_str();
    // `active_compaction_idx` was selected via `compaction_tail().is_some()`
    // above, so `compaction_tail()` is guaranteed to be `Some` here. We
    // still pattern-match (rather than `expect`) so a future refactor of
    // either the selection or the helper cannot turn a missed branch
    // into a runtime panic — falling back to chronological is always a
    // safe behaviour for the projection layer.
    let Some(tail_id) = messages[comp_idx].compaction_tail() else {
        return messages.iter().collect();
    };

    let summary_idx = messages
        .iter()
        .enumerate()
        .skip(comp_idx + 1)
        .find(|(_, m)| m.summary_parent() == Some(comp_id))
        .map(|(idx, _)| idx);

    let Some(summary_idx) = summary_idx else {
        return messages.iter().collect();
    };

    let tail_idx = messages.iter().position(|m| m.id == tail_id);
    let Some(tail_idx) = tail_idx else {
        return messages.iter().collect();
    };

    if !(tail_idx < comp_idx && comp_idx < summary_idx) {
        return messages.iter().collect();
    }

    let mut out: Vec<&MessageProjection> = Vec::with_capacity(messages.len());
    out.extend(messages[comp_idx..=summary_idx].iter());
    out.extend(messages[tail_idx..comp_idx].iter());
    out.extend(messages[summary_idx + 1..].iter());
    out
}

/// Outcome of [`prune_inline_tool_output`]. Borrowed when the input
/// did not need rewriting, owned when it did — avoids a copy on
/// the (common) under-threshold path.
#[derive(Debug, PartialEq, Eq)]
pub enum PruneResult<'a> {
    /// Tool output was within budget or the tool is in
    /// [`PRUNE_PROTECTED_TOOLS`]. No rewrite happened.
    Kept(&'a str),
    /// Tool output exceeded [`TOOL_OUTPUT_MAX_CHARS`] and was
    /// replaced with the placeholder. The caller substitutes this
    /// string for the original inline content.
    Pruned(String),
}

impl PruneResult<'_> {
    /// Render to an owned string the dispatcher can splice into
    /// the projection.
    pub fn into_string(self) -> String {
        match self {
            PruneResult::Kept(s) => s.to_string(),
            PruneResult::Pruned(s) => s,
        }
    }

    pub fn was_pruned(&self) -> bool {
        matches!(self, PruneResult::Pruned(_))
    }
}

/// Apply the inline-tool-output prune rule.
///
/// Returns [`PruneResult::Pruned`] when:
/// - `tool_name` is NOT in [`PRUNE_PROTECTED_TOOLS`], AND
/// - the **UTF-16 code-unit count** of `inline_content` is strictly
///   greater than [`TOOL_OUTPUT_MAX_CHARS`]. UTF-16 is the unit the
///   opencode upstream's JavaScript `String.length` exposes, so a
///   transcript that round-trips between Libra and opencode hits
///   the threshold at the same place. `chars().count()` (Unicode
///   scalar values) would silently under-prune for emoji and other
///   surrogate-pair code points; `len()` (bytes) would over-prune
///   for ASCII-light multi-byte text. UTF-16 is the only unit that
///   keeps both the threshold and the rendered `length=` attribute
///   in agreement with upstream.
///
/// In that case the rewritten string is
/// `<pruned attachment_id="..." length="...">` so the model still
/// sees the attachment entry point. `attachment_id = None` renders
/// the placeholder without an attachment id (still useful: tells
/// the model the original content existed but is no longer inline).
///
/// `length` in the placeholder is also reported in UTF-16 code
/// units — the same number the threshold compared against — so
/// downstream surfaces (replay, logs, cross-runtime debugging) see
/// the value they expect.
pub fn prune_inline_tool_output<'a>(
    tool_name: &str,
    inline_content: &'a str,
    attachment_id: Option<&str>,
) -> PruneResult<'a> {
    if PRUNE_PROTECTED_TOOLS.contains(&tool_name) {
        return PruneResult::Kept(inline_content);
    }
    let utf16_len = utf16_code_unit_count(inline_content);
    if utf16_len <= TOOL_OUTPUT_MAX_CHARS {
        return PruneResult::Kept(inline_content);
    }
    let placeholder = match attachment_id {
        Some(id) => format!(r#"<pruned attachment_id="{id}" length="{utf16_len}">"#),
        None => format!(r#"<pruned length="{utf16_len}">"#),
    };
    PruneResult::Pruned(placeholder)
}

/// UTF-16 code-unit count for `s`, matching JavaScript's
/// `String.prototype.length`. ASCII characters count as 1, BMP
/// non-ASCII as 1, supplementary-plane (emoji, rare CJK) as 2.
/// Used by [`prune_inline_tool_output`] to keep its threshold and
/// emitted `length=` attribute consistent with the opencode
/// upstream.
fn utf16_code_unit_count(s: &str) -> usize {
    s.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str) -> MessageProjection {
        MessageProjection::new(id, ProjectionKind::User)
    }
    fn assistant(id: &str) -> MessageProjection {
        MessageProjection::new(id, ProjectionKind::Assistant)
    }
    fn compaction(id: &str, tail_start_id: Option<&str>) -> MessageProjection {
        MessageProjection::new(
            id,
            ProjectionKind::Compaction {
                tail_start_id: tail_start_id.map(str::to_string),
            },
        )
    }
    fn summary(id: &str, parent: &str) -> MessageProjection {
        MessageProjection::new(
            id,
            ProjectionKind::Summary {
                parent_compaction_id: parent.to_string(),
            },
        )
    }

    fn ids<'a>(out: &[&'a MessageProjection]) -> Vec<&'a str> {
        out.iter().map(|m| m.id.as_str()).collect()
    }

    /// Doc-prescribed scenario 1: compaction with tail + matching
    /// summary triggers reorder. Expected output:
    /// `[compaction, summary, tail-1, rest]`.
    #[test]
    fn filter_compacted_reorders_when_tail_compaction_summary_present() {
        let msgs = vec![
            user("0"),
            assistant("1"),
            compaction("2", Some("1")),
            summary("3", "2"),
            user("4"),
            assistant("5"),
        ];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["2", "3", "1", "4", "5"]);
    }

    /// Doc-prescribed scenario 2: compaction with `tail_start_id =
    /// None` skips the reorder; the input is returned in
    /// chronological order. Note: the input itself does not include
    /// any pre-compaction message that would qualify as "tail",
    /// matching the doc's table.
    #[test]
    fn filter_compacted_skips_reorder_when_compaction_has_no_tail() {
        let msgs = vec![
            user("0"),
            assistant("1"),
            compaction("2", None),
            summary("3", "2"),
        ];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["0", "1", "2", "3"]);
    }

    /// Doc-prescribed scenario 3: summary appears BEFORE the
    /// compaction marker — this is malformed for our rule because
    /// the rule wants `tail < compaction < summary`. The function
    /// returns the chronological order unchanged.
    #[test]
    fn filter_compacted_skips_reorder_when_summary_predates_compaction() {
        // Construct a transcript where index 2 is a summary that
        // claims parent="0" (an earlier message) and index 3 is the
        // compaction with tail="1". Per the doc table this should
        // return `[0, 1, 2, 3]` — no reorder.
        let msgs = vec![
            user("0"),
            assistant("1"),
            summary("2", "0"),
            compaction("3", Some("1")),
        ];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["0", "1", "2", "3"]);
    }

    /// Defensive scenario: a compaction has a `tail_start_id` that
    /// does not resolve to any message in the slice. The reorder
    /// rule has nothing to anchor on, so chronological order wins.
    #[test]
    fn filter_compacted_skips_reorder_when_tail_id_does_not_resolve() {
        let msgs = vec![
            user("0"),
            assistant("1"),
            compaction("2", Some("does-not-exist")),
            summary("3", "2"),
        ];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["0", "1", "2", "3"]);
    }

    /// Defensive scenario: the summary's `parent_compaction_id` does
    /// not match the latest compaction. No pairing happens; the
    /// reorder is skipped.
    #[test]
    fn filter_compacted_skips_reorder_when_summary_parent_mismatches() {
        let msgs = vec![
            user("0"),
            assistant("1"),
            compaction("2", Some("1")),
            summary("3", "wrong-parent"),
        ];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["0", "1", "2", "3"]);
    }

    /// Edge case: an empty input returns an empty output without
    /// panicking. Equally, a transcript with no compaction marker
    /// at all is a no-op.
    #[test]
    fn filter_compacted_handles_empty_and_no_compaction() {
        assert!(filter_compacted(&[]).is_empty());
        let msgs = vec![user("0"), assistant("1"), user("2")];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["0", "1", "2"]);
    }

    /// Scenario: two compactions in the same transcript. The
    /// algorithm picks the **latest** compaction (idx 6, tail
    /// pointer "4") as the active one and pairs it with the
    /// summary at idx 7. Reorder yields
    /// `[6, 7]` (compaction marker + summary), then
    /// `[4, 5]` (the retained tail starting at the active
    /// compaction's tail pointer), then `[8]` (post-summary).
    /// Everything before the active tail boundary — including the
    /// earlier compaction at idx 2 and its summary at idx 3 — is
    /// dropped: the model only sees what the latest summary
    /// already covers, plus the retained tail, plus what came
    /// after. This matches opencode's `filterCompacted` semantics.
    #[test]
    fn filter_compacted_uses_latest_compaction_when_multiple_present() {
        let msgs = vec![
            user("0"),
            assistant("1"),
            compaction("2", Some("0")), // superseded by the later compaction
            summary("3", "2"),
            user("4"),
            assistant("5"),
            compaction("6", Some("4")), // active (latest) compaction
            summary("7", "6"),
            user("8"),
        ];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["6", "7", "4", "5", "8"]);
    }

    /// Prune scenario: a tool whose output exceeds the threshold
    /// gets rewritten to a `<pruned>` placeholder carrying the
    /// attachment id and the original UTF-16 code-unit length —
    /// the same unit that determined the prune decision.
    #[test]
    fn prune_replaces_oversized_unprotected_tool_output() {
        let big = "x".repeat(TOOL_OUTPUT_MAX_CHARS + 1);
        let result = prune_inline_tool_output("read_file", &big, Some("att-42"));
        assert!(result.was_pruned());
        let rendered = result.into_string();
        assert!(rendered.starts_with("<pruned attachment_id=\"att-42\""));
        // ASCII: UTF-16 length == chars == bytes, so all three
        // unit choices coincide for this fixture.
        assert!(rendered.contains(&format!("length=\"{}\"", big.len())));
    }

    /// Prune scenario: an oversized output WITHOUT an attachment id
    /// still gets pruned, but the placeholder omits the
    /// `attachment_id` attribute. The model still sees the length
    /// signal so it knows content existed.
    #[test]
    fn prune_replaces_oversized_output_without_attachment_id() {
        let big = "y".repeat(TOOL_OUTPUT_MAX_CHARS + 1);
        let result = prune_inline_tool_output("read_file", &big, None);
        let rendered = result.into_string();
        assert!(rendered.starts_with("<pruned length="));
        assert!(!rendered.contains("attachment_id"));
    }

    /// Prune scenario: a tool in [`PRUNE_PROTECTED_TOOLS`] is
    /// passed through verbatim even when oversized. Protects the
    /// `submit_intent_draft` and `submit_plan_draft` records the
    /// dispatcher relies on for intent state.
    #[test]
    fn prune_preserves_protected_tool_outputs_even_when_oversized() {
        let big = "z".repeat(TOOL_OUTPUT_MAX_CHARS * 2);
        for protected in ["skill", "submit_intent_draft", "submit_plan_draft"] {
            let result = prune_inline_tool_output(protected, &big, Some("att"));
            assert!(
                !result.was_pruned(),
                "protected tool {protected:?} must not be pruned"
            );
            assert_eq!(result.into_string(), big);
        }
    }

    /// Prune scenario: an under-threshold output is returned as
    /// `Kept`, borrowing the input string. No allocation happens.
    #[test]
    fn prune_keeps_short_tool_output() {
        let small = "x".repeat(TOOL_OUTPUT_MAX_CHARS);
        let result = prune_inline_tool_output("read_file", &small, Some("att"));
        assert!(!result.was_pruned());
        match result {
            PruneResult::Kept(s) => assert_eq!(s, small.as_str()),
            PruneResult::Pruned(_) => unreachable!(),
        }
    }

    /// Prune scenario: an output of exactly `TOOL_OUTPUT_MAX_CHARS`
    /// chars is kept (the threshold is `> max`, not `>= max`). The
    /// boundary check matters when the doc's `>` rule is read
    /// strictly.
    #[test]
    fn prune_threshold_is_strictly_greater_than() {
        let exactly = "a".repeat(TOOL_OUTPUT_MAX_CHARS);
        let result = prune_inline_tool_output("read_file", &exactly, None);
        assert!(!result.was_pruned());
    }

    /// Prune scenario: a transcript carrying emoji / supplementary-
    /// plane characters counts each emoji as **two** UTF-16 code
    /// units (matching opencode's JavaScript `String.length`). A
    /// `chars().count()` threshold would let
    /// `TOOL_OUTPUT_MAX_CHARS / 2 + 1` emoji slip through; the
    /// UTF-16 unit catches it. The rendered `length=` attribute
    /// reports the same UTF-16 number so a Libra-pruned placeholder
    /// reads the same way an opencode-pruned one would.
    ///
    /// Emoji alone are not enough to prove the implementation uses
    /// UTF-16 specifically (a bytes-based threshold would also
    /// over-count surrogate pairs). The companion test
    /// [`prune_keeps_bmp_multibyte_when_under_utf16_threshold`]
    /// covers the inverse — a BMP multi-byte string whose byte
    /// length exceeds the threshold but whose UTF-16 length is at
    /// the limit — to lock the unit choice in both directions.
    #[test]
    fn prune_uses_utf16_code_units_for_threshold_and_length() {
        // 🚀 is one Unicode scalar value but two UTF-16 code units.
        // Build a string whose UTF-16 length is exactly
        // TOOL_OUTPUT_MAX_CHARS + 2 (still over the threshold) but
        // whose `chars().count()` is only TOOL_OUTPUT_MAX_CHARS / 2 + 1
        // (under the threshold under chars-based comparison).
        let emoji_count = TOOL_OUTPUT_MAX_CHARS / 2 + 1;
        let big = "🚀".repeat(emoji_count);
        let utf16_len: usize = big.encode_utf16().count();
        assert!(utf16_len > TOOL_OUTPUT_MAX_CHARS);
        assert!(big.chars().count() <= TOOL_OUTPUT_MAX_CHARS);

        let result = prune_inline_tool_output("read_file", &big, Some("att-emoji"));
        assert!(
            result.was_pruned(),
            "UTF-16-oversized emoji content must prune even when chars().count() is under threshold"
        );
        let rendered = result.into_string();
        assert!(
            rendered.contains(&format!("length=\"{utf16_len}\"")),
            "placeholder must report UTF-16 code-unit length, got {rendered:?}"
        );
        // Sanity-check the length is NOT the byte length and NOT the
        // char count — those would be silent regressions.
        assert!(!rendered.contains(&format!("length=\"{}\"", big.len())));
        assert!(!rendered.contains(&format!("length=\"{}\"", big.chars().count())));
    }

    /// Prune scenario: a BMP multi-byte string whose UTF-16 length
    /// is exactly `TOOL_OUTPUT_MAX_CHARS` (so under the strict-`>`
    /// threshold) but whose UTF-8 byte length is roughly twice
    /// that. A bytes-based threshold would prune; the UTF-16
    /// threshold keeps the content. This is the inverse of the
    /// emoji case and locks the unit choice as UTF-16 rather than
    /// "anything that happens to over-count surrogate pairs".
    #[test]
    fn prune_keeps_bmp_multibyte_when_under_utf16_threshold() {
        // 'é' (U+00E9) is one Unicode scalar value, one UTF-16 code
        // unit, but two UTF-8 bytes. Build a string of exactly
        // TOOL_OUTPUT_MAX_CHARS é's: UTF-16 length is at the limit
        // (kept under `>` rule), byte length is ~2x the limit.
        let bmp = "é".repeat(TOOL_OUTPUT_MAX_CHARS);
        assert_eq!(bmp.encode_utf16().count(), TOOL_OUTPUT_MAX_CHARS);
        assert!(bmp.len() > TOOL_OUTPUT_MAX_CHARS);

        let result = prune_inline_tool_output("read_file", &bmp, Some("att"));
        assert!(
            !result.was_pruned(),
            "BMP-multi-byte content under the UTF-16 threshold must NOT prune even though its byte length exceeds TOOL_OUTPUT_MAX_CHARS"
        );
    }

    /// Defensive scenario: a malformed transcript where the active
    /// compaction's `tail_start_id` points at the compaction marker
    /// itself (self-reference). `tail_idx == comp_idx` fails the
    /// `tail_idx < comp_idx` check, so the projection serves
    /// chronological order without panicking. The dispatcher still
    /// gets a stable transcript even when its own state is
    /// inconsistent.
    #[test]
    fn filter_compacted_handles_self_referencing_tail_start_id() {
        let msgs = vec![
            user("0"),
            assistant("1"),
            compaction("2", Some("2")),
            summary("3", "2"),
        ];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["0", "1", "2", "3"]);
    }

    /// Defensive scenario: the active compaction sits at the very
    /// end of the transcript with no following summary (e.g. the
    /// runtime crashed before writing the summary). The
    /// summary-search loop returns `None`, so the projection serves
    /// chronological order. Without this guard a future refactor
    /// could try to splice past the end and panic.
    #[test]
    fn filter_compacted_handles_compaction_with_no_following_summary() {
        let msgs = vec![
            user("0"),
            assistant("1"),
            compaction("2", Some("1")),
            // No summary — runtime crashed before writing one.
        ];
        let out = filter_compacted(&msgs);
        assert_eq!(ids(&out), vec!["0", "1", "2"]);
    }
}
