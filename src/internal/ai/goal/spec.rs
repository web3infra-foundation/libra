//! Goal specification — the immutable "what" the user asked for.
//!
//! Per `docs/improvement/opencode.md` lines 537-555, a [`GoalSpec`] is the
//! immutable seed of an active Goal: objective, acceptance criteria,
//! constraints, evidence policy, budget, and provenance. It pins down what
//! the supervisor must drive toward and what the deterministic verifier
//! must check before allowing `Completed`.
//!
//! Everything in this file is plain data + JSON round-trip helpers. There
//! is **no** logic that decides whether a Goal *can* be created (that
//! gating belongs to the CLI/TUI/Control entry points in P6.5/P6.6) and no
//! logic that decides whether the Goal *is done* (that belongs to the
//! verifier in P6.2). Keeping the schema pure makes it easy to reason
//! about replay determinism: the same JSON wire bytes always reconstitute
//! the same `GoalSpec`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Hard upper bound on the `objective` field length, in **bytes** of the
/// canonical UTF-8 encoding. Sized to fit comfortably inside a single
/// model preamble line while still allowing a multi-paragraph natural
/// objective. Enforced by [`GoalSpec::new`] and [`GoalSpec::with_objective`].
pub const MAX_OBJECTIVE_LEN: usize = 16 * 1024;

/// Who created or acted on this Goal.
///
/// Goals must be created by an explicit actor — `docs/improvement/opencode.md`
/// line 594 forbids inferring a Goal from a casual user message. The
/// supervisor, the deterministic verifier, the cancel path, and the audit
/// log all consume this enum to attribute actions.
///
/// `Automation` is gated by `docs/improvement/opencode.md` line 596 to
/// callers that hold the current controller lease + explicitly declare
/// `goal = true`; the gate itself lives in the Code Control surface
/// (P6.6), not here.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalActor {
    /// Local interactive user (CLI prompt or TUI).
    User { id: Option<String> },
    /// Code Control automation that holds the current controller lease.
    Automation { agent_id: String, lease_id: String },
    /// Internal Libra runtime path (resume / migration / replay).
    System { reason: String },
}

/// One acceptance criterion the verifier (P6.2) checks before allowing
/// `Completed`. Mirrors `docs/improvement/opencode.md` lines 550-555.
///
/// `required = true` criteria all need to appear in
/// `GoalState.completed_criteria` AND each one needs at least one
/// evidence ref. `required = false` criteria are nice-to-have hints
/// (e.g. cosmetic acceptance signals); they do not block completion.
///
/// `verifier_hint` is opt-in advisory text shown to the supervisor so it
/// can craft a precise continuation prompt when the criterion is still
/// pending — e.g. `"check that tests pass via cargo test --lib"`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalCriterion {
    pub id: String,
    pub description: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_hint: Option<String>,
}

/// What kinds of evidence the verifier accepts for a Goal.
///
/// Some Goals (e.g. "draft a release note") have no executable artefact
/// and must rely on human-written explanations; others (e.g.
/// "add a unit test") fail completion if no `git status` shows changes.
/// Per `docs/improvement/opencode.md` line 677-680 the verifier consults
/// this policy to decide which gates to apply.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalEvidencePolicy {
    /// Default: at least one evidence ref per required criterion AND
    /// (if any criterion describes a code change) a VCS state evidence.
    #[default]
    Standard,
    /// Documentation-only or analysis-only Goals: human-written
    /// explanation in `verification` is sufficient. The verifier still
    /// requires every required criterion to be claimed.
    DocumentationOnly,
}

/// Budget envelope the supervisor enforces over the Goal's lifetime.
///
/// Per `docs/improvement/opencode.md` lines 660-661, hitting `hard_cap`
/// puts the Goal into `Blocked { reason: BudgetApprovalRequired }`,
/// **never** `Completed` or `Cancelled`. `warn_threshold` is purely
/// informative: the supervisor surfaces a TUI hint without changing
/// status.
///
/// Costs are denominated in micro-USD (1e-6 USD) so the schema avoids
/// floating-point drift across replay; conversion to display-friendly
/// dollars happens at the rendering boundary (P6.5).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalBudget {
    /// Total spend cap in micro-USD. Hitting this transitions the Goal
    /// to `Blocked { reason: BudgetApprovalRequired }`. `0` means the
    /// caller did not set a hard cap; the supervisor falls back to its
    /// `[code.budget]` defaults (configured in P5.3 / P5.4).
    pub hard_cap_micro_usd: u64,
    /// First-warn threshold in micro-USD. The supervisor emits a single
    /// advisory event the moment the running cost crosses this value;
    /// it never re-emits within the same Goal. `0` disables warnings.
    pub warn_threshold_micro_usd: u64,
    /// Wall-clock cap (seconds). Hitting this transitions the Goal to
    /// `Blocked { reason: WallClockExpired }`. `0` disables.
    pub wall_clock_seconds: u64,
    /// Maximum number of supervisor continuation loops before the Goal
    /// stops auto-progressing and waits for the user. Per
    /// `docs/improvement/opencode.md` line 667 this surfaces as
    /// `Blocked { reason: LoopLimitNeedsUser }`, never `Completed`.
    pub max_continuation_loops: u32,
}

impl Default for GoalBudget {
    fn default() -> Self {
        Self {
            hard_cap_micro_usd: 0,
            warn_threshold_micro_usd: 0,
            wall_clock_seconds: 0,
            max_continuation_loops: 16,
        }
    }
}

/// Errors surfaced from [`GoalSpec::new`] / [`GoalSpec::with_objective`].
/// Pure data validation — anything that depends on the surrounding
/// session, registered tools, or the budget config lives in the
/// supervisor / control entry points.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GoalSpecError {
    #[error(
        "GoalSpec.objective must not be blank — Goal mode requires an explicit objective string"
    )]
    EmptyObjective,
    #[error(
        "GoalSpec.objective is {actual} bytes which exceeds the {max}-byte cap; \
         shorten the objective and add detail through `acceptance_criteria` instead"
    )]
    ObjectiveTooLong { actual: usize, max: usize },
    #[error(
        "GoalSpec.acceptance_criteria contains duplicate id `{id}` — \
         each criterion id must be unique within a Goal so the verifier can match \
         completion claims unambiguously"
    )]
    DuplicateCriterionId { id: String },
    #[error(
        "GoalSpec.acceptance_criteria[{index}].id must not be blank — \
         criterion ids are surfaced verbatim in completion claims"
    )]
    BlankCriterionId { index: usize },
}

/// Immutable specification of an active Goal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GoalSpec {
    pub goal_id: Uuid,
    pub thread_id: String,
    pub session_id: String,
    pub objective: String,
    pub acceptance_criteria: Vec<GoalCriterion>,
    pub constraints: Vec<String>,
    #[serde(default)]
    pub evidence_policy: GoalEvidencePolicy,
    #[serde(default)]
    pub budget: GoalBudget,
    pub created_at: DateTime<Utc>,
    pub created_by: GoalActor,
}

impl GoalSpec {
    /// Build a new spec, validating objective + criterion ids.
    ///
    /// `goal_id` is required up front so the caller controls the id
    /// (e.g. for control-plane handoff). The deterministic verifier
    /// reads the same id from every emitted [`super::event::GoalEvent`]
    /// to attribute events back to the spec.
    ///
    /// Argument count exceeds clippy's default cap because every field
    /// of [`GoalSpec`] is independently load-bearing — splitting into
    /// builder structs would obscure the validation contract; allowed
    /// per the documented exception.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        goal_id: Uuid,
        thread_id: impl Into<String>,
        session_id: impl Into<String>,
        objective: impl Into<String>,
        acceptance_criteria: Vec<GoalCriterion>,
        constraints: Vec<String>,
        evidence_policy: GoalEvidencePolicy,
        budget: GoalBudget,
        created_at: DateTime<Utc>,
        created_by: GoalActor,
    ) -> Result<Self, GoalSpecError> {
        let objective = objective.into();
        validate_objective(&objective)?;
        validate_criteria(&acceptance_criteria)?;
        Ok(Self {
            goal_id,
            thread_id: thread_id.into(),
            session_id: session_id.into(),
            objective,
            acceptance_criteria,
            constraints,
            evidence_policy,
            budget,
            created_at,
            created_by,
        })
    }

    /// Replace the objective. Validates the new value the same way as
    /// [`Self::new`] so a malformed string never reaches downstream
    /// consumers.
    pub fn with_objective(self, objective: impl Into<String>) -> Result<Self, GoalSpecError> {
        let objective = objective.into();
        validate_objective(&objective)?;
        Ok(Self { objective, ..self })
    }
}

fn validate_objective(objective: &str) -> Result<(), GoalSpecError> {
    if objective.trim().is_empty() {
        return Err(GoalSpecError::EmptyObjective);
    }
    let actual = objective.len();
    if actual > MAX_OBJECTIVE_LEN {
        return Err(GoalSpecError::ObjectiveTooLong {
            actual,
            max: MAX_OBJECTIVE_LEN,
        });
    }
    Ok(())
}

/// Validate a slice of acceptance criteria for the same shape rules
/// [`GoalSpec::new`] enforces on construction:
///
/// 1. No blank id (after trimming whitespace).
/// 2. No duplicate id within the slice.
///
/// Re-exported as [`validate_criteria`] for downstream consumers
/// (`apply` in `state.rs` runs this on every
/// [`super::event::GoalEvent::CriteriaRevised`] payload so a
/// malformed revision cannot land in `GoalState`).
pub(super) fn validate_criteria(criteria: &[GoalCriterion]) -> Result<(), GoalSpecError> {
    use std::collections::HashSet;
    let mut seen: HashSet<&str> = HashSet::new();
    for (index, criterion) in criteria.iter().enumerate() {
        if criterion.id.trim().is_empty() {
            return Err(GoalSpecError::BlankCriterionId { index });
        }
        if !seen.insert(criterion.id.as_str()) {
            return Err(GoalSpecError::DuplicateCriterionId {
                id: criterion.id.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_actor() -> GoalActor {
        GoalActor::User {
            id: Some("test-user".to_string()),
        }
    }

    fn fixture_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-08T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn rejects_blank_objective() {
        let err = GoalSpec::new(
            Uuid::new_v4(),
            "thread-1",
            "session-1",
            "   ",
            Vec::new(),
            Vec::new(),
            GoalEvidencePolicy::Standard,
            GoalBudget::default(),
            fixture_now(),
            fixture_actor(),
        )
        .expect_err("blank objective must fail");
        assert_eq!(err, GoalSpecError::EmptyObjective);
    }

    #[test]
    fn rejects_oversized_objective() {
        let big = "a".repeat(MAX_OBJECTIVE_LEN + 1);
        let err = GoalSpec::new(
            Uuid::new_v4(),
            "thread-1",
            "session-1",
            big,
            Vec::new(),
            Vec::new(),
            GoalEvidencePolicy::Standard,
            GoalBudget::default(),
            fixture_now(),
            fixture_actor(),
        )
        .expect_err("objective beyond cap must fail");
        assert_eq!(
            err,
            GoalSpecError::ObjectiveTooLong {
                actual: MAX_OBJECTIVE_LEN + 1,
                max: MAX_OBJECTIVE_LEN
            }
        );
    }

    #[test]
    fn rejects_duplicate_criterion_ids() {
        let crit = |id: &str| GoalCriterion {
            id: id.to_string(),
            description: "x".to_string(),
            required: true,
            verifier_hint: None,
        };
        let err = GoalSpec::new(
            Uuid::new_v4(),
            "thread-1",
            "session-1",
            "do the thing",
            vec![crit("a"), crit("a")],
            Vec::new(),
            GoalEvidencePolicy::Standard,
            GoalBudget::default(),
            fixture_now(),
            fixture_actor(),
        )
        .expect_err("duplicate ids must fail");
        assert_eq!(
            err,
            GoalSpecError::DuplicateCriterionId {
                id: "a".to_string()
            }
        );
    }

    #[test]
    fn rejects_blank_criterion_id() {
        let err = GoalSpec::new(
            Uuid::new_v4(),
            "thread-1",
            "session-1",
            "do the thing",
            vec![GoalCriterion {
                id: "  ".to_string(),
                description: "x".to_string(),
                required: true,
                verifier_hint: None,
            }],
            Vec::new(),
            GoalEvidencePolicy::Standard,
            GoalBudget::default(),
            fixture_now(),
            fixture_actor(),
        )
        .expect_err("blank id must fail");
        assert_eq!(err, GoalSpecError::BlankCriterionId { index: 0 });
    }

    /// JSON round-trip pins the wire shape so future readers (Code
    /// Control NDJSON, CLI snapshot, control plane handoff) stay
    /// byte-stable. A regression that adds an undocumented field
    /// surfaces here.
    #[test]
    fn json_round_trip_preserves_every_field() {
        let spec = GoalSpec::new(
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            "thread-1",
            "session-1",
            "deliver feature X",
            vec![GoalCriterion {
                id: "tests-pass".to_string(),
                description: "cargo test --lib green".to_string(),
                required: true,
                verifier_hint: Some("cargo test --lib".to_string()),
            }],
            vec!["no destructive git ops".to_string()],
            GoalEvidencePolicy::Standard,
            GoalBudget {
                hard_cap_micro_usd: 5_000_000,
                warn_threshold_micro_usd: 2_000_000,
                wall_clock_seconds: 1_800,
                max_continuation_loops: 24,
            },
            fixture_now(),
            fixture_actor(),
        )
        .expect("happy-path spec must construct cleanly");
        let json = serde_json::to_string(&spec).expect("serialize");
        let back: GoalSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(spec, back);
    }

    /// A criterion deserialized from a payload that omits
    /// `verifier_hint` deserialises as `None` (skip_serializing_if +
    /// serde default for Option). This protects upgrades that add a
    /// criterion with no hint from breaking older consumers.
    #[test]
    fn criterion_deserializes_without_optional_hint() {
        let json = r#"{"id":"c1","description":"x","required":true}"#;
        let crit: GoalCriterion = serde_json::from_str(json).expect("deserialize");
        assert!(crit.verifier_hint.is_none());
    }
}
