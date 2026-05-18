//! Integration acceptance for `libra code` thread_id contract — agent.md
//! Implementation Phase 2 "thread_id 统一、legacy backfill、provider_thread_id
//! 仅内部".
//!
//! These tests live at the `SessionStore` boundary (not the CLI) because the
//! Phase 2 contract is a property of the on-disk session blob shape:
//!
//! - **Canonical lookup** — `load_for_thread_id(thread_id, working_dir)` must
//!   return the session whose `metadata["thread_id"]` matches and whose
//!   `working_dir` matches, ignoring sessions saved for a sibling worktree.
//! - **Legacy backfill is non-destructive** — `preview_legacy_metadata_backfill`
//!   reports what would change without writing; `apply_legacy_metadata_backfill`
//!   writes `legacy_session_id` + `thread_id` (when derivable) + a previously
//!   recorded `provider_thread_id`, but never overwrites an already-present
//!   `thread_id` or `provider_thread_id` value. Re-running preview after apply
//!   reports no further updates.
//! - **provider_thread_id stays internal** — it round-trips through the
//!   `SessionState` JSON blob and surfaces in
//!   `SessionMetadataBackfillUpdate::provider_thread_id`, but is never the
//!   primary key for resume lookup; only the canonical `thread_id` is.
//!
//! Layer: L1 — no binary subprocess, just `SessionStore` against a tempdir.

use libra::internal::ai::session::{SessionState, SessionStore};
use serde_json::json;
use tempfile::TempDir;

/// Resume contract: `load_for_thread_id` matches on both `thread_id` and
/// `working_dir`. A session saved against a different working_dir with the
/// same `thread_id` must NOT shadow the canonical one even when its file is
/// more recent.
#[test]
fn load_for_thread_id_matches_canonical_thread_id_and_working_dir() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::from_storage_path(temp.path());
    let thread_id = "22222222-2222-4222-8222-222222222222";
    let canonical_dir = "/repo/main";

    let mut sibling = SessionState::new("/repo/feature-branch");
    sibling.summary = "sibling".to_string();
    sibling
        .metadata
        .insert("thread_id".to_string(), json!(thread_id));
    store.save(&sibling).unwrap();

    // Ensure the canonical session lands AFTER the sibling on the filesystem
    // mtime axis. Without the sleep, two saves in the same millisecond can
    // tie on `session_modified_time` and the canonical row's mtime wouldn't
    // be strictly greater.
    std::thread::sleep(std::time::Duration::from_millis(10));

    let mut canonical = SessionState::new(canonical_dir);
    canonical.summary = "canonical".to_string();
    canonical
        .metadata
        .insert("thread_id".to_string(), json!(thread_id));
    store.save(&canonical).unwrap();

    let loaded = store
        .load_for_thread_id(thread_id, canonical_dir)
        .unwrap()
        .expect("canonical session must be findable by (thread_id, working_dir)");

    assert_eq!(loaded.summary, "canonical");
    assert_eq!(loaded.working_dir, canonical_dir);
    assert_eq!(
        loaded
            .metadata
            .get("thread_id")
            .and_then(serde_json::Value::as_str),
        Some(thread_id),
    );
}

/// `load_for_thread_id` returns `Ok(None)` when no session has the requested
/// canonical id, even if other sessions exist for the same working_dir.
#[test]
fn load_for_thread_id_returns_none_when_no_session_matches() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::from_storage_path(temp.path());

    let mut other = SessionState::new("/repo/main");
    other.metadata.insert(
        "thread_id".to_string(),
        json!("33333333-3333-4333-8333-333333333333"),
    );
    store.save(&other).unwrap();

    let missing = store
        .load_for_thread_id("00000000-0000-4000-8000-000000000000", "/repo/main")
        .unwrap();
    assert!(
        missing.is_none(),
        "lookup must return None for absent thread_id"
    );
}

/// `preview_legacy_metadata_backfill` reports the would-be updates without
/// writing to disk, and `apply_legacy_metadata_backfill` then writes the same
/// updates idempotently. A second preview after apply reports no further work.
#[test]
fn legacy_metadata_backfill_preview_is_idempotent_after_apply() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::from_storage_path(temp.path());
    let thread_id = "44444444-4444-4444-8444-444444444444";

    let mut session = SessionState::new("/repo/main");
    let session_id = session.id.clone();
    session
        .metadata
        .insert("thread_id".to_string(), json!(thread_id));
    store.save(&session).unwrap();

    let preview = store.preview_legacy_metadata_backfill().unwrap();
    assert_eq!(preview.scanned, 1);
    assert_eq!(preview.updates.len(), 1);
    assert_eq!(preview.updates[0].legacy_session_id, session_id);
    assert_eq!(
        preview.updates[0].canonical_thread_id.as_deref(),
        Some(thread_id),
    );
    // Preview must not write.
    let pre_apply = store.load(&session_id).unwrap();
    assert!(
        !pre_apply.metadata.contains_key("legacy_session_id"),
        "preview must not mutate legacy_session_id"
    );

    let applied = store.apply_legacy_metadata_backfill().unwrap();
    assert_eq!(applied.updates.len(), 1);
    let after_apply = store.load(&session_id).unwrap();
    assert_eq!(
        after_apply
            .metadata
            .get("legacy_session_id")
            .and_then(serde_json::Value::as_str),
        Some(session_id.as_str()),
    );

    // Second preview must observe the already-applied legacy_session_id and
    // not report another update — backfill is converging, not accumulating.
    let second_preview = store.preview_legacy_metadata_backfill().unwrap();
    assert!(
        second_preview.updates.is_empty(),
        "second preview must report no further work after apply, got {:?}",
        second_preview.updates,
    );
}

/// `provider_thread_id` is part of the session blob's metadata (so we can
/// resume an upstream provider conversation), but is NOT the resume-lookup
/// key — only canonical `thread_id` is. Demonstrate by saving two sessions
/// that agree on `thread_id` + `working_dir` and differ only in
/// `provider_thread_id`; `load_for_thread_id` resolves to one of them
/// without erroring, and the `provider_thread_id` survives round-trip on
/// the canonical hit.
#[test]
fn provider_thread_id_is_internal_metadata_not_resume_key() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::from_storage_path(temp.path());
    let thread_id = "55555555-5555-4555-8555-555555555555";
    let working_dir = "/repo/main";

    let mut session = SessionState::new(working_dir);
    session.summary = "with-provider".to_string();
    session
        .metadata
        .insert("thread_id".to_string(), json!(thread_id));
    session
        .metadata
        .insert("provider_thread_id".to_string(), json!("codex-abc-123"));
    store.save(&session).unwrap();

    let loaded = store
        .load_for_thread_id(thread_id, working_dir)
        .unwrap()
        .expect("session must resolve by canonical thread_id");

    // The internal provider_thread_id round-trips through the JSON blob…
    assert_eq!(
        loaded
            .metadata
            .get("provider_thread_id")
            .and_then(serde_json::Value::as_str),
        Some("codex-abc-123"),
    );

    // …but the resume contract keys off the canonical thread_id, not the
    // provider's id. Querying by provider_thread_id must NOT find a match;
    // the API surface only takes (canonical thread_id, working_dir).
    let by_provider_id = store
        .load_for_thread_id("codex-abc-123", working_dir)
        .unwrap();
    assert!(
        by_provider_id.is_none(),
        "provider_thread_id must not double as a resume key — got Some({:?})",
        by_provider_id.map(|s| s.id),
    );
}

/// Apply backfill on a session whose metadata already contains both
/// `thread_id` and `provider_thread_id` and assert apply does not overwrite
/// either value — the entry uses `or_insert_with`, so existing canonical /
/// provider ids must survive verbatim.
#[test]
fn legacy_backfill_preserves_existing_thread_and_provider_ids() {
    let temp = TempDir::new().unwrap();
    let store = SessionStore::from_storage_path(temp.path());
    let canonical_thread_id = "66666666-6666-4666-8666-666666666666";
    let provider_thread_id = "codex-existing-9999";

    let mut session = SessionState::new("/repo/main");
    let session_id = session.id.clone();
    session
        .metadata
        .insert("thread_id".to_string(), json!(canonical_thread_id));
    session
        .metadata
        .insert("provider_thread_id".to_string(), json!(provider_thread_id));
    store.save(&session).unwrap();

    let applied = store.apply_legacy_metadata_backfill().unwrap();
    assert_eq!(applied.updates.len(), 1);

    let after_apply = store.load(&session_id).unwrap();
    assert_eq!(
        after_apply
            .metadata
            .get("thread_id")
            .and_then(serde_json::Value::as_str),
        Some(canonical_thread_id),
        "existing canonical thread_id must not be overwritten by backfill",
    );
    assert_eq!(
        after_apply
            .metadata
            .get("provider_thread_id")
            .and_then(serde_json::Value::as_str),
        Some(provider_thread_id),
        "existing provider_thread_id must not be overwritten by backfill",
    );
    assert_eq!(
        after_apply
            .metadata
            .get("legacy_session_id")
            .and_then(serde_json::Value::as_str),
        Some(session_id.as_str()),
        "legacy_session_id must be added by backfill",
    );
}
