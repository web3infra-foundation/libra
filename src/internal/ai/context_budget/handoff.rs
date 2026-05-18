//! Context handoff types + 8-section SUMMARY parser.
//!
//! This module is the OC-Phase 4 P4.3 deliverable from
//! `docs/improvement/opencode.md`. The dispatcher (OC-Phase 3) and the
//! compaction agent (OC-Phase 4 P4.4) both need a way to describe a
//! point-in-time **handoff** of session context — what the model has
//! done, what is still in flight, what the caller should pick up
//! next — without dragging the full chat transcript through.
//!
//! The doc mandates a [literal 8-section SUMMARY template] for the
//! `summary` field. Missing any section is a hard schema error so a
//! compaction agent that drops a heading is not silently accepted as
//! "good enough" — the runtime refuses, instead of falling back to
//! raw transcript and risking a context overflow on the next call.
//!
//! What this module owns:
//! - [`ContextHandoff`] struct with the doc-mandated six fields
//!   (`summary`, `recent_tail`, `attachment_refs`, `source_frame_id`,
//!   `remaining_budget_tokens`, `created_at`).
//! - [`parse_handoff_template`] strict parser.
//! - [`ContextHandoffParseError`] for the three failure modes
//!   ([`SchemaMismatch`](ContextHandoffParseError::SchemaMismatch),
//!   [`OutOfOrder`](ContextHandoffParseError::OutOfOrder), and
//!   [`DuplicateHeading`](ContextHandoffParseError::DuplicateHeading)).
//!
//! What this module is **not**:
//! - It does not call the compaction agent. P4.4 wires the
//!   `compaction.md` embedded prompt into a model run and feeds the
//!   result through this parser.
//! - It does not decide *when* to compact. The
//!   [`crate::internal::ai::context_budget::ContextBudget`] surfaces
//!   that signal; this module only validates the produced summary.
//!
//! [literal 8-section SUMMARY template]:
//! https://github.com/genedna/libra/blob/main/docs/improvement/opencode.md#literal-summary-template

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::frame::{ContextAttachmentRef, ContextFrameSegment};

/// Structured snapshot the dispatcher hands a sub-agent (or that the
/// compaction loop hands to the next round) instead of the raw chat
/// history.
///
/// Field semantics (verbatim from
/// `docs/improvement/opencode.md`):
///
/// - `summary` — the 8-section markdown produced by the compaction
///   agent. Validated by [`parse_handoff_template`] before this struct
///   is written, so a populated [`ContextHandoff`] is guaranteed to
///   carry every required heading.
/// - `recent_tail` — the last N raw [`ContextFrameSegment`]s that
///   belong to the post-summary "retained tail" (cf. `filterCompacted`
///   ordering rule, OC-Phase 4 P4.5).
/// - `attachment_refs` — file / blob references the summary cites so
///   the next agent can re-read them on demand. Materialised by
///   [`crate::internal::ai::context_budget::frame::ContextFrameEvent::attachment_refs`].
/// - `source_frame_id` — the frame the summary was built from, so
///   replay can match the transcript without traversing JSONL.
/// - `remaining_budget_tokens` — output of the budget calculator at
///   the moment the handoff was produced. The receiving runtime uses
///   it to size the next request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextHandoff {
    pub summary: String,
    pub recent_tail: Vec<ContextFrameSegment>,
    pub attachment_refs: Vec<ContextAttachmentRef>,
    pub source_frame_id: Uuid,
    pub remaining_budget_tokens: u64,
    pub created_at: DateTime<Utc>,
}

/// Failure modes produced by [`parse_handoff_template`] (and by the
/// dispatcher when it refuses to forward a handoff with a malformed
/// summary).
///
/// Kept in this module — not in `sub_agent.rs` — because the parser
/// is the one authority that produces them. OC-Phase 4 P4.4 wires
/// the dispatcher into this parser; until then there is no
/// duplicate / bridge type to keep in sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextHandoffParseError {
    /// One or more required headings are missing from the summary.
    /// The list walks the canonical template top-to-bottom so a
    /// mixed top-level / progress-subsection regression is reported
    /// in true template order (e.g. a missing `### Done` between a
    /// present `## Progress` and a missing `## Key Decisions`
    /// surfaces as `["### Done", "## Key Decisions"]`).
    SchemaMismatch { missing_sections: Vec<String> },
    /// All required headings are present but at least one is out of
    /// canonical template order. Doc rule: the compaction agent must
    /// emit headings literally, in order, so a reorder is treated as
    /// a schema violation rather than a tolerated reshuffle.
    /// `observed` is what we saw, `expected` is the canonical order
    /// (filtered to the same scope as `observed`); for top-level
    /// reorders the entries are `## Name`, for progress-subsection
    /// reorders they are `### Name`.
    OutOfOrder {
        observed: Vec<String>,
        expected: Vec<String>,
    },
    /// A canonical heading appeared twice. Silently merging two
    /// `## Goal` blocks (or two `### Done` blocks under the same
    /// `## Progress`) would let a compaction agent split content
    /// across non-adjacent occurrences, defeating the literal-template
    /// guarantee. The string carries the doubled heading verbatim
    /// (`## Name` or `### Name`) so the dispatcher can echo it back.
    DuplicateHeading { heading: String },
}

impl std::fmt::Display for ContextHandoffParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SchemaMismatch { missing_sections } => write!(
                f,
                "context handoff summary is missing required section(s): {}",
                missing_sections.join(", ")
            ),
            Self::OutOfOrder { observed, expected } => write!(
                f,
                "context handoff summary headings are out of canonical order: observed [{}], expected [{}]",
                observed.join(", "),
                expected.join(", ")
            ),
            Self::DuplicateHeading { heading } => write!(
                f,
                "context handoff summary contains duplicate heading: {heading}"
            ),
        }
    }
}

impl std::error::Error for ContextHandoffParseError {}

/// Bullet-list content under one section heading. The parser strips
/// the leading `-` marker and any literal `(none)` placeholder so
/// callers can iterate on real entries directly. Empty `bullets` is
/// allowed (e.g. when the section legitimately had only `(none)`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ParsedSection {
    pub bullets: Vec<String>,
}

/// Output of [`parse_handoff_template`]. Each section is exposed as a
/// named field so callers can reach for `parsed.goal.bullets[0]`
/// without digging into a `HashMap` lookup that loses static type
/// guarantees.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ParsedSummary {
    pub goal: ParsedSection,
    pub constraints_and_preferences: ParsedSection,
    pub progress_done: ParsedSection,
    pub progress_in_progress: ParsedSection,
    pub progress_blocked: ParsedSection,
    pub key_decisions: ParsedSection,
    pub next_steps: ParsedSection,
    pub critical_context: ParsedSection,
    pub relevant_files: ParsedSection,
}

/// Required `## ...` heading literals in canonical order.
const REQUIRED_TOP_SECTIONS: &[&str] = &[
    "Goal",
    "Constraints & Preferences",
    "Progress",
    "Key Decisions",
    "Next Steps",
    "Critical Context",
    "Relevant Files",
];

/// Required `### ...` sub-headings under `## Progress`.
const REQUIRED_PROGRESS_SUBSECTIONS: &[&str] = &["Done", "In Progress", "Blocked"];

/// Validate and parse the SUMMARY template.
///
/// The parser walks `summary` line-by-line and:
/// 1. Recognises any `## <name>` heading at column 0 whose name
///    appears in [`REQUIRED_TOP_SECTIONS`]. Indented headings are
///    rejected — the doc's literal-template rule means the
///    compaction agent must emit headings flush-left.
/// 2. Recognises any `### <name>` heading under `## Progress` whose
///    name appears in [`REQUIRED_PROGRESS_SUBSECTIONS`].
/// 3. Collects bullet entries (lines starting with `- `) under the
///    most-recently-opened section. The literal `(none)` placeholder
///    is stripped so callers iterate on real content only.
/// 4. After the walk, verifies every required heading was present.
///    Missing headings surface as
///    [`ContextHandoffParseError::SchemaMismatch`] with the list
///    walking the canonical template top-to-bottom (top-level and
///    progress-subsection gaps **interleave** in true template
///    order — e.g. a missing `### Done` lands between a present
///    `## Progress` and a missing `## Key Decisions` rather than
///    after every top-level gap).
/// 5. If every heading is present, verifies headings appear in
///    canonical template order. Reorders surface as
///    [`ContextHandoffParseError::OutOfOrder`].
///
/// The parser tolerates trailing whitespace on heading lines (the
/// `\r` in CRLF, plus stray spaces a markdown formatter may insert)
/// but is strict about heading names and column-0 placement: a
/// heading like `## Goals` (extra `s`) does NOT match `Goal` and the
/// section is reported as missing.
pub fn parse_handoff_template(summary: &str) -> Result<ParsedSummary, ContextHandoffParseError> {
    let mut parsed = ParsedSummary::default();
    let mut active = ActiveSection::None;

    // Insertion-ordered observation logs so we can both check
    // presence and check ordering against the canonical template.
    let mut observed_top: Vec<&'static str> = Vec::new();
    let mut observed_progress_sub: Vec<&'static str> = Vec::new();

    for raw in summary.lines() {
        // Trim only the trailing side: CRLF's `\r` plus any
        // markdown-linter-inserted trailing whitespace. Leading
        // whitespace is preserved so `   ## Goal` (indented) does
        // NOT match the literal-template rule.
        let line = raw.trim_end();

        if let Some(top) = strip_heading(line, "## ") {
            // Top-level heading. Switch context only when it is one of
            // the canonical names; unknown `## ...` lines are skipped
            // (the parser stays liberal about bonus content the
            // compaction agent might emit) but still reset `active`
            // to None so following bullets do not bleed into the
            // previously-active section.
            if let Some(matched) = match_canonical_exact(top, REQUIRED_TOP_SECTIONS) {
                if observed_top.contains(&matched) {
                    return Err(ContextHandoffParseError::DuplicateHeading {
                        heading: format!("## {matched}"),
                    });
                }
                observed_top.push(matched);
                active = ActiveSection::from_top(matched);
                continue;
            } else {
                active = ActiveSection::None;
                continue;
            }
        }

        if let Some(sub) = strip_heading(line, "### ") {
            // Any `### ...` line is a heading boundary regardless of
            // whether the parser knows the name — we reset `active`
            // first so an orphan subsection (e.g. `### Done` under
            // `## Goal`, or a typo'd `### Did`) cannot merge its
            // following bullets into the prior section. We then
            // re-promote `active` only when the heading is a
            // canonical progress sub-section appearing under
            // `## Progress`.
            let inside_progress = matches!(
                active,
                ActiveSection::ProgressContainer
                    | ActiveSection::ProgressDone
                    | ActiveSection::ProgressInProgress
                    | ActiveSection::ProgressBlocked
            );
            if inside_progress
                && let Some(matched) = match_canonical_exact(sub, REQUIRED_PROGRESS_SUBSECTIONS)
            {
                if observed_progress_sub.contains(&matched) {
                    return Err(ContextHandoffParseError::DuplicateHeading {
                        heading: format!("### {matched}"),
                    });
                }
                observed_progress_sub.push(matched);
                active = ActiveSection::from_progress_sub(matched);
                continue;
            }
            // Orphan or non-canonical `### ...` heading: discard the
            // active section so the following bullet block does not
            // attach to a stale parent.
            active = ActiveSection::None;
            continue;
        }

        // Bullet line under the active section. Bullet indentation
        // is permissive: a list under a heading may legally indent
        // for nested items, so we trim_start before matching `- `.
        let trimmed_for_bullet = line.trim_start();
        if let Some(bullet) = strip_bullet(trimmed_for_bullet) {
            let body = bullet.trim();
            if body.is_empty() || body.eq_ignore_ascii_case("(none)") {
                continue;
            }
            let owner = active.section_mut(&mut parsed);
            if let Some(section) = owner {
                section.bullets.push(body.to_string());
            }
        }
    }

    // Assemble missing list by walking the canonical template
    // top-to-bottom. When `## Progress` is seen, the three required
    // `### ...` subsections interleave at that position so the
    // emitted ordering matches what a reader scanning the template
    // would expect.
    let mut missing: Vec<String> = Vec::new();
    for required in REQUIRED_TOP_SECTIONS {
        if !observed_top.contains(required) {
            missing.push(format!("## {required}"));
            continue;
        }
        if *required == "Progress" {
            for sub in REQUIRED_PROGRESS_SUBSECTIONS {
                if !observed_progress_sub.contains(sub) {
                    missing.push(format!("### {sub}"));
                }
            }
        }
    }
    if !missing.is_empty() {
        return Err(ContextHandoffParseError::SchemaMismatch {
            missing_sections: missing,
        });
    }

    // All required headings are present — now enforce canonical
    // order. Compare the observation log against the canonical list;
    // because every required heading is present, the lengths match
    // and a reorder will surface as a non-equal element.
    if observed_top.as_slice() != REQUIRED_TOP_SECTIONS {
        return Err(ContextHandoffParseError::OutOfOrder {
            observed: observed_top
                .into_iter()
                .map(|name| format!("## {name}"))
                .collect(),
            expected: REQUIRED_TOP_SECTIONS
                .iter()
                .map(|name| format!("## {name}"))
                .collect(),
        });
    }
    if observed_progress_sub.as_slice() != REQUIRED_PROGRESS_SUBSECTIONS {
        return Err(ContextHandoffParseError::OutOfOrder {
            observed: observed_progress_sub
                .into_iter()
                .map(|name| format!("### {name}"))
                .collect(),
            expected: REQUIRED_PROGRESS_SUBSECTIONS
                .iter()
                .map(|name| format!("### {name}"))
                .collect(),
        });
    }

    Ok(parsed)
}

/// Tracks which section the parser is currently filling. Nested sub-
/// sections of `## Progress` are flat enum variants for cheap match
/// dispatch in the bullet-attachment hot path.
#[derive(Clone, Copy)]
enum ActiveSection {
    None,
    Goal,
    ConstraintsAndPreferences,
    /// `## Progress` heading observed but no `###` yet.
    ProgressContainer,
    ProgressDone,
    ProgressInProgress,
    ProgressBlocked,
    KeyDecisions,
    NextSteps,
    CriticalContext,
    RelevantFiles,
}

impl ActiveSection {
    fn from_top(name: &'static str) -> Self {
        match name {
            "Goal" => Self::Goal,
            "Constraints & Preferences" => Self::ConstraintsAndPreferences,
            "Progress" => Self::ProgressContainer,
            "Key Decisions" => Self::KeyDecisions,
            "Next Steps" => Self::NextSteps,
            "Critical Context" => Self::CriticalContext,
            "Relevant Files" => Self::RelevantFiles,
            _ => Self::None,
        }
    }

    fn from_progress_sub(name: &'static str) -> Self {
        match name {
            "Done" => Self::ProgressDone,
            "In Progress" => Self::ProgressInProgress,
            "Blocked" => Self::ProgressBlocked,
            _ => Self::None,
        }
    }

    fn section_mut(self, parsed: &mut ParsedSummary) -> Option<&mut ParsedSection> {
        match self {
            Self::Goal => Some(&mut parsed.goal),
            Self::ConstraintsAndPreferences => Some(&mut parsed.constraints_and_preferences),
            Self::ProgressDone => Some(&mut parsed.progress_done),
            Self::ProgressInProgress => Some(&mut parsed.progress_in_progress),
            Self::ProgressBlocked => Some(&mut parsed.progress_blocked),
            Self::KeyDecisions => Some(&mut parsed.key_decisions),
            Self::NextSteps => Some(&mut parsed.next_steps),
            Self::CriticalContext => Some(&mut parsed.critical_context),
            Self::RelevantFiles => Some(&mut parsed.relevant_files),
            // ProgressContainer / None: bullets at this level are
            // ignored — the doc requires a `### subsection` before
            // bullets land under `## Progress`.
            _ => None,
        }
    }
}

/// Match `name` (heading text after `## ` / `### `) against the
/// canonical list. The match is byte-exact: no lower-cased fallback,
/// no punctuation normalisation, no whitespace trimming on either
/// side. Any trailing whitespace on the original line was already
/// removed by [`str::trim_end`] before this function runs.
fn match_canonical_exact(name: &str, allowed: &[&'static str]) -> Option<&'static str> {
    allowed.iter().copied().find(|candidate| *candidate == name)
}

/// Strip the heading prefix iff the line starts with it at column 0.
/// Indented headings — `   ## Goal` — return `None` so the parser
/// can either ignore them or report the canonical heading as
/// missing. CRLF endings are tolerated because the caller has
/// already removed trailing whitespace via `trim_end`.
fn strip_heading<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)
}

fn strip_bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix('-').filter(|rest| rest.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical 8-section template populated with placeholder
    /// content. Used as the happy-path baseline for parser tests.
    const CANONICAL_FILLED: &str = "\
## Goal
- Add unit test for utils::path::join

## Constraints & Preferences
- Stick to the existing snapshot harness

## Progress
### Done
- Located the helper in src/utils/path.rs

### In Progress
- Drafting the failure-mode case

### Blocked
- (none)

## Key Decisions
- Use proptest for random separators

## Next Steps
- Wire the new test module into mod.rs

## Critical Context
- Existing test runner does not propagate panics

## Relevant Files
- src/utils/path.rs: target of the new test
- tests/utils/path_test.rs: new test fixture
";

    /// Scenario: the canonical template parses cleanly and every
    /// section receives the correct bullets. `(none)` placeholders
    /// are dropped so callers iterate on real content only.
    #[test]
    fn parse_handoff_template_accepts_canonical_template() {
        let parsed = parse_handoff_template(CANONICAL_FILLED).unwrap();
        assert_eq!(
            parsed.goal.bullets,
            vec!["Add unit test for utils::path::join".to_string()]
        );
        assert_eq!(
            parsed.constraints_and_preferences.bullets,
            vec!["Stick to the existing snapshot harness".to_string()]
        );
        assert_eq!(
            parsed.progress_done.bullets,
            vec!["Located the helper in src/utils/path.rs".to_string()]
        );
        assert_eq!(
            parsed.progress_in_progress.bullets,
            vec!["Drafting the failure-mode case".to_string()]
        );
        assert!(
            parsed.progress_blocked.bullets.is_empty(),
            "(none) placeholder must produce an empty bullets list"
        );
        assert_eq!(
            parsed.key_decisions.bullets,
            vec!["Use proptest for random separators".to_string()]
        );
        assert_eq!(
            parsed.next_steps.bullets,
            vec!["Wire the new test module into mod.rs".to_string()]
        );
        assert_eq!(
            parsed.critical_context.bullets,
            vec!["Existing test runner does not propagate panics".to_string()]
        );
        assert_eq!(parsed.relevant_files.bullets.len(), 2);
        assert!(parsed.relevant_files.bullets[0].starts_with("src/utils/path.rs"));
    }

    /// Scenario: dropping `## Critical Context` surfaces it as the
    /// only missing section in the error. The runtime uses this list
    /// to render a precise "compaction agent missed: ..." prompt to
    /// the user.
    #[test]
    fn parse_handoff_template_reports_single_missing_top_section() {
        let stripped = CANONICAL_FILLED.replace(
            "## Critical Context\n- Existing test runner does not propagate panics\n",
            "",
        );
        let err = parse_handoff_template(&stripped).unwrap_err();
        match err {
            ContextHandoffParseError::SchemaMismatch { missing_sections } => {
                assert_eq!(missing_sections, vec!["## Critical Context".to_string()]);
            }
            other => panic!("expected SchemaMismatch variant, got {other:?}"),
        }
    }

    /// Scenario: dropping the `### In Progress` sub-section under a
    /// present `## Progress` block surfaces only the sub-section in
    /// the missing list. Top-level `## Progress` itself is NOT listed
    /// as missing because the parser saw it.
    #[test]
    fn parse_handoff_template_reports_missing_progress_subsection() {
        let stripped =
            CANONICAL_FILLED.replace("### In Progress\n- Drafting the failure-mode case\n\n", "");
        let err = parse_handoff_template(&stripped).unwrap_err();
        match err {
            ContextHandoffParseError::SchemaMismatch { missing_sections } => {
                assert_eq!(missing_sections, vec!["### In Progress".to_string()]);
            }
            other => panic!("expected SchemaMismatch variant, got {other:?}"),
        }
    }

    /// Scenario: removing the entire `## Progress` block surfaces the
    /// top section as missing AND does NOT cascade to listing the
    /// three sub-sections. The runtime should not double-report a
    /// nested gap when the parent is gone.
    #[test]
    fn parse_handoff_template_does_not_cascade_when_progress_section_is_absent() {
        let stripped = CANONICAL_FILLED
            .lines()
            .filter(|line| {
                !line.starts_with("## Progress")
                    && !line.starts_with("### Done")
                    && !line.starts_with("### In Progress")
                    && !line.starts_with("### Blocked")
                    && !matches!(
                        *line,
                        "- Located the helper in src/utils/path.rs"
                            | "- Drafting the failure-mode case"
                            | "- (none)"
                    )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let err = parse_handoff_template(&stripped).unwrap_err();
        match err {
            ContextHandoffParseError::SchemaMismatch { missing_sections } => {
                assert_eq!(missing_sections, vec!["## Progress".to_string()]);
            }
            other => panic!("expected SchemaMismatch variant, got {other:?}"),
        }
    }

    /// Scenario: a typo'd heading (`## Goals` instead of `## Goal`)
    /// triggers the schema mismatch — the doc rule is byte-equal
    /// canonical names. The typo'd section is reported as the
    /// missing one (the spelling variant is silently dropped, not
    /// promoted).
    #[test]
    fn parse_handoff_template_rejects_typo_in_heading() {
        let typo = CANONICAL_FILLED.replace("## Goal\n", "## Goals\n");
        let err = parse_handoff_template(&typo).unwrap_err();
        match err {
            ContextHandoffParseError::SchemaMismatch { missing_sections } => {
                assert!(missing_sections.contains(&"## Goal".to_string()));
            }
            other => panic!("expected SchemaMismatch variant, got {other:?}"),
        }
    }

    /// Scenario: the missing-section list is canonical order, not
    /// observed order. Removing both `## Goal` (first heading) and
    /// `## Relevant Files` (last heading) puts them in the natural
    /// reading order in the error.
    #[test]
    fn parse_handoff_template_missing_list_is_canonical_order() {
        let stripped: String = CANONICAL_FILLED
            .lines()
            .filter(|line| {
                !line.starts_with("## Goal")
                    && *line != "- Add unit test for utils::path::join"
                    && !line.starts_with("## Relevant Files")
                    && *line != "- src/utils/path.rs: target of the new test"
                    && *line != "- tests/utils/path_test.rs: new test fixture"
            })
            .collect::<Vec<_>>()
            .join("\n");
        let err = parse_handoff_template(&stripped).unwrap_err();
        match err {
            ContextHandoffParseError::SchemaMismatch { missing_sections } => {
                assert_eq!(
                    missing_sections,
                    vec!["## Goal".to_string(), "## Relevant Files".to_string(),]
                );
            }
            other => panic!("expected SchemaMismatch variant, got {other:?}"),
        }
    }

    /// Scenario: bullets without the leading `- ` (e.g. plain prose
    /// or `* `) are ignored. The parser is bullet-strict so prose
    /// noise from a hallucinating compaction agent does not leak
    /// into the structured handoff output.
    #[test]
    fn parse_handoff_template_ignores_non_dash_bullet_lines() {
        let template = "\
## Goal
This is prose, not a bullet.
- Real bullet

## Constraints & Preferences
* Asterisk bullets are NOT recognised
- Allowed bullet

## Progress
### Done
- Done item
### In Progress
- (none)
### Blocked
- (none)

## Key Decisions
- Decided

## Next Steps
- Step

## Critical Context
- Context

## Relevant Files
- file.rs: x
";
        let parsed = parse_handoff_template(template).unwrap();
        assert_eq!(parsed.goal.bullets, vec!["Real bullet".to_string()]);
        assert_eq!(
            parsed.constraints_and_preferences.bullets,
            vec!["Allowed bullet".to_string()]
        );
    }

    /// Scenario: `Display` on the error renders all missing sections
    /// joined by `, ` so it round-trips into a single user-facing
    /// log line.
    #[test]
    fn schema_mismatch_display_lists_every_missing_section() {
        let err = ContextHandoffParseError::SchemaMismatch {
            missing_sections: vec!["## Goal".to_string(), "### Done".to_string()],
        };
        let formatted = format!("{err}");
        assert!(formatted.contains("## Goal"));
        assert!(formatted.contains("### Done"));
        assert!(formatted.contains(", "));
    }

    /// Scenario: when both a top-level section that comes AFTER
    /// `## Progress` (e.g. `## Key Decisions`) and a progress
    /// sub-section (e.g. `### Done`) are missing, the missing-list
    /// must walk the canonical template top-to-bottom — putting the
    /// `### Done` gap between `## Progress` and `## Key Decisions`,
    /// not at the end. Without this interleaving rule, all
    /// top-level gaps would be reported before any progress-sub
    /// gaps regardless of position.
    #[test]
    fn parse_handoff_template_interleaves_top_and_progress_subsection_gaps() {
        let stripped = CANONICAL_FILLED
            .replace(
                "### Done\n- Located the helper in src/utils/path.rs\n\n",
                "",
            )
            .replace(
                "## Key Decisions\n- Use proptest for random separators\n",
                "",
            );
        let err = parse_handoff_template(&stripped).unwrap_err();
        match err {
            ContextHandoffParseError::SchemaMismatch { missing_sections } => {
                assert_eq!(
                    missing_sections,
                    vec!["### Done".to_string(), "## Key Decisions".to_string()],
                    "progress-sub gap must precede the later top-level gap in canonical order"
                );
            }
            other => panic!("expected SchemaMismatch variant, got {other:?}"),
        }
    }

    /// Scenario: every required heading is present but `## Progress`
    /// has been moved before `## Constraints & Preferences`. The
    /// parser must reject this as `OutOfOrder` because the doc
    /// requires the literal template — including its ordering.
    #[test]
    fn parse_handoff_template_rejects_reordered_top_sections() {
        let reordered = "\
## Goal
- A

## Progress
### Done
- (none)
### In Progress
- (none)
### Blocked
- (none)

## Constraints & Preferences
- B

## Key Decisions
- C

## Next Steps
- D

## Critical Context
- E

## Relevant Files
- f.rs: g
";
        let err = parse_handoff_template(reordered).unwrap_err();
        match err {
            ContextHandoffParseError::OutOfOrder { observed, expected } => {
                // Observed has Progress before Constraints; expected
                // has Constraints before Progress.
                assert_eq!(observed[0], "## Goal");
                assert_eq!(observed[1], "## Progress");
                assert_eq!(observed[2], "## Constraints & Preferences");
                assert_eq!(expected[1], "## Constraints & Preferences");
                assert_eq!(expected[2], "## Progress");
            }
            other => panic!("expected OutOfOrder, got {other:?}"),
        }
    }

    /// Scenario: every required `## ...` heading is present in
    /// canonical order but `### Blocked` appears before `### Done`
    /// inside `## Progress`. The parser must report the sub-section
    /// reorder via `OutOfOrder`, with `### Name` shaped strings.
    #[test]
    fn parse_handoff_template_rejects_reordered_progress_subsections() {
        let reordered = "\
## Goal
- A

## Constraints & Preferences
- B

## Progress
### Blocked
- (none)
### Done
- D
### In Progress
- (none)

## Key Decisions
- C

## Next Steps
- D

## Critical Context
- E

## Relevant Files
- f.rs: g
";
        let err = parse_handoff_template(reordered).unwrap_err();
        match err {
            ContextHandoffParseError::OutOfOrder { observed, expected } => {
                assert_eq!(observed[0], "### Blocked");
                assert_eq!(observed[1], "### Done");
                assert_eq!(observed[2], "### In Progress");
                assert_eq!(
                    expected,
                    vec![
                        "### Done".to_string(),
                        "### In Progress".to_string(),
                        "### Blocked".to_string(),
                    ]
                );
            }
            other => panic!("expected OutOfOrder, got {other:?}"),
        }
    }

    /// Scenario: a heading indented by leading whitespace
    /// (`   ## Goal`) does NOT match the canonical literal.
    /// The parser reports `## Goal` as missing — flush-left
    /// placement is part of the doc's literal-template rule.
    #[test]
    fn parse_handoff_template_rejects_indented_heading() {
        let indented = CANONICAL_FILLED.replace("## Goal\n", "   ## Goal\n");
        let err = parse_handoff_template(&indented).unwrap_err();
        match err {
            ContextHandoffParseError::SchemaMismatch { missing_sections } => {
                assert!(
                    missing_sections.contains(&"## Goal".to_string()),
                    "indented `   ## Goal` must surface as missing, got {missing_sections:?}"
                );
            }
            other => panic!("expected SchemaMismatch variant, got {other:?}"),
        }
    }

    /// Scenario: a CRLF-terminated heading line still matches the
    /// canonical literal. The parser strips trailing whitespace
    /// before matching so Windows line endings round-trip cleanly.
    #[test]
    fn parse_handoff_template_accepts_crlf_line_endings() {
        let crlf = CANONICAL_FILLED.replace('\n', "\r\n");
        let parsed = parse_handoff_template(&crlf).unwrap();
        assert_eq!(
            parsed.goal.bullets,
            vec!["Add unit test for utils::path::join".to_string()]
        );
    }

    /// Scenario: a duplicate top-level canonical heading triggers
    /// the `DuplicateHeading` error. The compaction agent must not
    /// split a section across non-adjacent occurrences — silent
    /// merge would defeat the literal-template guarantee.
    #[test]
    fn parse_handoff_template_rejects_duplicate_top_heading() {
        let duplicated = CANONICAL_FILLED.replace(
            "## Constraints & Preferences\n",
            "## Constraints & Preferences\n- B1\n## Goal\n- B2\n",
        );
        let err = parse_handoff_template(&duplicated).unwrap_err();
        match err {
            ContextHandoffParseError::DuplicateHeading { heading } => {
                assert_eq!(heading, "## Goal");
            }
            other => panic!("expected DuplicateHeading, got {other:?}"),
        }
    }

    /// Scenario: a duplicate `### Done` under the same `## Progress`
    /// triggers `DuplicateHeading`. Same rationale as the top-level
    /// case — silent merge would let an agent split done items
    /// across two non-adjacent blocks.
    #[test]
    fn parse_handoff_template_rejects_duplicate_progress_subsection() {
        let duplicated = CANONICAL_FILLED.replace(
            "### In Progress\n- Drafting the failure-mode case\n",
            "### In Progress\n- Drafting the failure-mode case\n### Done\n- Second\n",
        );
        let err = parse_handoff_template(&duplicated).unwrap_err();
        match err {
            ContextHandoffParseError::DuplicateHeading { heading } => {
                assert_eq!(heading, "### Done");
            }
            other => panic!("expected DuplicateHeading, got {other:?}"),
        }
    }

    /// Scenario: a `### Done` heading appearing under a non-Progress
    /// section (here `## Goal`) is treated as an orphan: the parser
    /// resets `active` so the following bullets do NOT attach to the
    /// preceding section. Without this guard, malformed templates
    /// could silently merge unrelated bullets into the wrong field.
    #[test]
    fn parse_handoff_template_orphan_subsection_does_not_leak_bullets() {
        let template = "\
## Goal
- Real goal bullet
### Done
- This bullet must not attach to ## Goal

## Constraints & Preferences
- B

## Progress
### Done
- D
### In Progress
- (none)
### Blocked
- (none)

## Key Decisions
- C

## Next Steps
- D

## Critical Context
- E

## Relevant Files
- f.rs: g
";
        let parsed = parse_handoff_template(template).unwrap();
        assert_eq!(
            parsed.goal.bullets,
            vec!["Real goal bullet".to_string()],
            "orphan ### Done under ## Goal must reset active so the following bullet does NOT attach to goal"
        );
        // The orphan-block bullet is silently dropped (active == None
        // after the orphan heading) which is exactly the desired
        // outcome — it does not corrupt any canonical section.
        assert_eq!(parsed.progress_done.bullets, vec!["D".to_string()]);
    }

    /// Scenario: `DuplicateHeading::Display` includes the doubled
    /// heading text so the user-facing error tells the agent
    /// exactly which line to remove.
    #[test]
    fn duplicate_heading_display_includes_heading_text() {
        let err = ContextHandoffParseError::DuplicateHeading {
            heading: "## Goal".to_string(),
        };
        let formatted = format!("{err}");
        assert!(formatted.contains("## Goal"));
        assert!(formatted.contains("duplicate"));
    }

    /// Scenario: `OutOfOrder::Display` renders both observed and
    /// expected slices so a user can immediately see which heading
    /// moved. Same round-trip rationale as the SchemaMismatch
    /// display test above.
    #[test]
    fn out_of_order_display_lists_observed_and_expected() {
        let err = ContextHandoffParseError::OutOfOrder {
            observed: vec!["## Goal".to_string(), "## Progress".to_string()],
            expected: vec![
                "## Goal".to_string(),
                "## Constraints & Preferences".to_string(),
            ],
        };
        let formatted = format!("{err}");
        assert!(formatted.contains("observed"));
        assert!(formatted.contains("expected"));
        assert!(formatted.contains("## Progress"));
        assert!(formatted.contains("## Constraints & Preferences"));
    }

    #[test]
    fn context_handoff_parse_error_display_pins_each_variant() {
        assert_eq!(
            ContextHandoffParseError::SchemaMismatch {
                missing_sections: vec!["### Done".to_string(), "## Key Decisions".to_string()],
            }
            .to_string(),
            "context handoff summary is missing required section(s): ### Done, ## Key Decisions",
        );
        assert_eq!(
            ContextHandoffParseError::OutOfOrder {
                observed: vec!["## Progress".to_string(), "## Goal".to_string()],
                expected: vec!["## Goal".to_string(), "## Progress".to_string()],
            }
            .to_string(),
            "context handoff summary headings are out of canonical order: \
             observed [## Progress, ## Goal], expected [## Goal, ## Progress]",
        );
        assert_eq!(
            ContextHandoffParseError::DuplicateHeading {
                heading: "## Goal".to_string(),
            }
            .to_string(),
            "context handoff summary contains duplicate heading: ## Goal",
        );
    }
}
