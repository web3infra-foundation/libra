use chrono::{Duration, Utc};
use libra::internal::ai::{
    context_budget::{
        MemoryAnchorConfidence, MemoryAnchorDraft, MemoryAnchorEvent, MemoryAnchorKind,
        MemoryAnchorReplay, MemoryAnchorReviewState, MemoryAnchorScope,
    },
    prompt::SystemPromptBuilder,
    session::jsonl::{SessionEvent, SessionJsonlStore},
};

#[test]
fn memory_anchor_events_roundtrip_and_replay_lifecycle() {
    let tmp = tempfile::TempDir::new().unwrap();
    let jsonl = SessionJsonlStore::new(tmp.path().join("session-1"));

    let draft = MemoryAnchorEvent::draft(MemoryAnchorDraft::session_user_constraint(
        "Never use mock DBs for integration tests.",
        "agent",
    ));
    jsonl
        .append(&SessionEvent::memory_anchor(draft.clone()))
        .unwrap();

    let replay = jsonl.load_memory_anchors().unwrap();
    let anchor = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    assert_eq!(anchor.review_state, MemoryAnchorReviewState::Draft);
    assert!(replay.active_anchors_at(Utc::now()).is_empty());

    let confirm = MemoryAnchorEvent::confirm(&anchor, Some("user confirmed".to_string()));
    jsonl.append(&SessionEvent::memory_anchor(confirm)).unwrap();
    let replay = jsonl.load_memory_anchors().unwrap();
    let active = replay.active_anchors_at(Utc::now());
    assert_eq!(active.len(), 1);
    assert_eq!(
        active[0].content,
        "Never use mock DBs for integration tests."
    );

    let revoke = MemoryAnchorEvent::revoke(&active[0], Some("no longer true".to_string()));
    jsonl.append(&SessionEvent::memory_anchor(revoke)).unwrap();
    let replay = jsonl.load_memory_anchors().unwrap();
    assert!(replay.active_anchors_at(Utc::now()).is_empty());
    let revoked = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    assert_eq!(revoked.review_state, MemoryAnchorReviewState::Revoked);
}

#[test]
fn memory_anchor_supersede_replay_points_old_anchor_to_replacement() {
    let mut replay = MemoryAnchorReplay::default();
    let draft = MemoryAnchorEvent::draft(MemoryAnchorDraft::session_user_constraint(
        "Prefer cargo test for validation.",
        "agent",
    ));
    replay.apply_event(draft.clone());
    let old = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();

    let replacement = MemoryAnchorEvent::draft(MemoryAnchorDraft::session_user_constraint(
        "Prefer cargo test --all for validation.",
        "agent",
    ));
    let supersede = MemoryAnchorEvent::supersede(
        &old,
        replacement.anchor_id,
        Some("more specific validation policy".to_string()),
    );
    replay.apply_event(replacement.clone());
    replay.apply_event(supersede);

    let old = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    assert_eq!(old.review_state, MemoryAnchorReviewState::Superseded);
    assert_eq!(old.superseded_by, Some(replacement.anchor_id));
}

#[test]
fn prompt_builder_includes_only_confirmed_active_memory_anchors() {
    let tmp = tempfile::TempDir::new().unwrap();
    let confirmed = confirmed_anchor(
        "Always preserve user-authored dirty worktree changes.",
        None,
    );
    let draft = replayed_anchor(MemoryAnchorEvent::draft(
        MemoryAnchorDraft::session_user_constraint("draft should not appear", "agent"),
    ));
    let revoked = {
        let anchor = confirmed_anchor("revoked should not appear", None);
        replayed_anchor(MemoryAnchorEvent::revoke(
            &anchor,
            Some("revoked".to_string()),
        ))
    };
    let expired = confirmed_anchor(
        "expired should not appear",
        Some(Utc::now() - Duration::minutes(1)),
    );

    let prompt = SystemPromptBuilder::new(tmp.path())
        .with_memory_anchors(vec![confirmed, draft, revoked, expired])
        .build();

    assert!(prompt.contains("## Memory Anchors"));
    assert!(prompt.contains("Always preserve user-authored dirty worktree changes."));
    assert!(!prompt.contains("draft should not appear"));
    assert!(!prompt.contains("revoked should not appear"));
    assert!(!prompt.contains("expired should not appear"));
}

fn confirmed_anchor(
    content: &str,
    expires_at: Option<chrono::DateTime<Utc>>,
) -> libra::internal::ai::context_budget::MemoryAnchor {
    let draft = MemoryAnchorEvent::draft(MemoryAnchorDraft {
        kind: MemoryAnchorKind::ProjectInvariant,
        content: content.to_string(),
        source_event_id: None,
        confidence: MemoryAnchorConfidence::High,
        scope: MemoryAnchorScope::Session,
        created_by: "agent".to_string(),
        expires_at,
    });
    let anchor = replayed_anchor(draft);
    replayed_anchor(MemoryAnchorEvent::confirm(
        &anchor,
        Some("confirmed".to_string()),
    ))
}

fn replayed_anchor(event: MemoryAnchorEvent) -> libra::internal::ai::context_budget::MemoryAnchor {
    let mut replay = MemoryAnchorReplay::default();
    replay.apply_event(event);
    replay.anchors().into_iter().next().unwrap()
}

#[test]
fn memory_anchor_replay_rejects_stale_event_for_same_anchor() {
    // INVARIANT (agent.md 2026-05-04 follow-up (f)): out-of-order replay of
    // an older `recorded_at` event must not regress an anchor that has
    // already advanced past it. Without the monotonicity guard, a delayed
    // `Drafted` event arriving after a `Confirmed` event would silently
    // un-confirm the anchor.
    let mut replay = MemoryAnchorReplay::default();
    let draft = MemoryAnchorEvent::draft(MemoryAnchorDraft::session_user_constraint(
        "Always run integration tests before merging.",
        "agent",
    ));
    replay.apply_event(draft.clone());
    let anchor = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();

    // Confirm the anchor with a later wall-clock timestamp.
    let mut confirm = MemoryAnchorEvent::confirm(&anchor, Some("user confirmed".to_string()));
    confirm.recorded_at = draft.recorded_at + Duration::seconds(5);
    replay.apply_event(confirm.clone());
    assert_eq!(
        replay
            .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
            .unwrap()
            .review_state,
        MemoryAnchorReviewState::Confirmed,
    );

    // Replay the original draft (which has a strictly older recorded_at).
    // The projection MUST stay at `Confirmed`.
    replay.apply_event(draft);
    let still_confirmed = replay
        .find_unique_by_prefix(&anchor.anchor_id.to_string()[..8])
        .unwrap();
    assert_eq!(
        still_confirmed.review_state,
        MemoryAnchorReviewState::Confirmed,
        "stale draft event must not regress the projection past Confirmed",
    );
}

#[test]
fn memory_anchor_replay_idempotent_on_equal_recorded_at() {
    // INVARIANT: re-applying the same event (identical `recorded_at`) is
    // allowed — JSONL crash-recovery replays can deliver the tail twice and
    // the projection must converge on the same state without rejecting the
    // duplicate.
    let mut replay = MemoryAnchorReplay::default();
    let draft = MemoryAnchorEvent::draft(MemoryAnchorDraft::session_user_constraint(
        "Always prefer cargo nextest where available.",
        "agent",
    ));
    replay.apply_event(draft.clone());
    let after_first = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    replay.apply_event(draft.clone());
    let after_second = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    assert_eq!(after_first, after_second);
}
