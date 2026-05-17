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
fn memory_anchor_replay_rejects_events_with_older_recorded_at() {
    // Apply a draft, confirm it, then re-apply the original draft event. The
    // duplicated draft has an older `recorded_at` than the confirm and must
    // not roll the anchor's review state back to Draft.
    let mut replay = MemoryAnchorReplay::default();
    let draft = MemoryAnchorEvent::draft(MemoryAnchorDraft::session_user_constraint(
        "Use real DBs for integration tests.",
        "agent",
    ));
    replay.apply_event(draft.clone());

    let anchor = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    let confirm = MemoryAnchorEvent::confirm(&anchor, Some("user confirmed".to_string()));
    replay.apply_event(confirm.clone());

    let confirmed = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    assert_eq!(confirmed.review_state, MemoryAnchorReviewState::Confirmed);

    // Replay the original (older) draft event again. The replay must skip
    // it because its `recorded_at` is older than the confirm's.
    replay.apply_event(draft.clone());
    let still_confirmed = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    assert_eq!(
        still_confirmed.review_state,
        MemoryAnchorReviewState::Confirmed,
        "older-or-equal recorded_at must not roll the projection backwards"
    );

    // An event with the same recorded_at is also skipped (duplicate
    // delivery on retry, same logical state).
    let mut same_recorded_at = confirm.clone();
    same_recorded_at.content = "tampered content that must be ignored".to_string();
    same_recorded_at.review_state = MemoryAnchorReviewState::Revoked;
    same_recorded_at.updated_at = confirm.updated_at + Duration::seconds(1);
    same_recorded_at.reason = Some("tamper attempt".to_string());
    replay.apply_event(same_recorded_at);
    let still_unchanged = replay
        .find_unique_by_prefix(&draft.anchor_id.to_string()[..8])
        .unwrap();
    assert_eq!(
        still_unchanged.review_state,
        MemoryAnchorReviewState::Confirmed,
        "equal-recorded_at event must be treated as a duplicate"
    );
    assert_eq!(
        still_unchanged.content, "Use real DBs for integration tests.",
        "tampered content must not overwrite the existing anchor"
    );
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
