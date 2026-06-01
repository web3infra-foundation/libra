//! CEX-S2-16 (Step 2.6) — `/agents` TUI pane observability.
//!
//! The card's verification target `cargo test ai_subagent_ui`. These tests pin
//! the read/replay contract of the agent-run pane: it is built purely from the
//! persisted `AgentRun` snapshots + `RunUsage` records under the session's
//! sessions root, so a pane rebuilt after a cache wipe (or process restart)
//! renders byte-for-byte identically (CEX-S2-16 验收 (5)), and the persisted
//! per-run usage surfaces as the `tokens` / `cost` columns (验收 (1)).

use libra::internal::{
    ai::agent_run::{
        AgentRun, AgentRunId, AgentRunStatus, AgentTaskId, RunUsage,
        event_store::AgentRunEventStore,
    },
    tui::format_agent_run_pane_with_usage,
};
use tempfile::tempdir;
use uuid::Uuid;

/// Persist a run snapshot plus its terminal usage record under `sessions_root`.
fn persist_run(
    store: &AgentRunEventStore,
    thread_id: Uuid,
    id: u128,
    status: AgentRunStatus,
    usage: Option<RunUsage>,
) -> AgentRunId {
    let run_id = AgentRunId(Uuid::from_u128(id));
    let run = AgentRun {
        id: run_id,
        task_id: AgentTaskId(Uuid::from_u128(id)),
        thread_id,
        provider: "deepseek".to_string(),
        model: "deepseek-chat".to_string(),
        transcript_path: format!("agents/{}.jsonl", run_id.0),
        workspace_path: None,
        status,
    };
    store
        .write_snapshot(thread_id, &run)
        .expect("persist run snapshot");
    if let Some(usage) = usage {
        store
            .write_run_usage(thread_id, run_id, &usage)
            .expect("persist run usage");
    }
    run_id
}

/// Render the pane from whatever a *fresh* store reads off disk — exactly how
/// the App rebuilds it (`format_agent_runs_pane`).
fn render_from_disk(sessions_root: &std::path::Path) -> String {
    let store = AgentRunEventStore::new(sessions_root);
    let runs = store.list_all_snapshots().expect("list snapshots");
    format_agent_run_pane_with_usage(&runs, |run| {
        store.read_run_usage(run.thread_id, run.id).ok().flatten()
    })
}

/// 验收 (1): the persisted per-run `RunUsage` surfaces as `tokens` / `cost`.
#[test]
fn agents_pane_shows_persisted_token_usage_and_cost() {
    let temp = tempdir().unwrap();
    let sessions_root = temp.path().join(".libra").join("sessions");
    let store = AgentRunEventStore::new(&sessions_root);
    let thread_id = Uuid::from_u128(0xfeed);

    let usage = RunUsage {
        prompt_tokens: 120,
        completion_tokens: 60,
        cost_estimate_micro_dollars: 1_500,
        ..RunUsage::default()
    };
    persist_run(&store, thread_id, 1, AgentRunStatus::Completed, Some(usage));

    let pane = render_from_disk(&sessions_root);
    assert!(pane.contains("Agent runs:"), "pane header: {pane}");
    assert!(
        pane.contains("tokens") && pane.contains("cost"),
        "columns: {pane}"
    );
    // 120 + 60 = 180 total tokens; 1_500 micro-dollars = $0.0015.
    assert!(pane.contains("180"), "total tokens must render: {pane}");
    assert!(
        pane.contains("$0.0015"),
        "cost estimate must render: {pane}"
    );
}

/// 验收 (5): rebuild after a cache wipe yields a byte-identical pane. The pane
/// reads only the on-disk snapshot/usage records — there is no hidden in-memory
/// state — so two independent reads (with a `.libra/cache/` deletion between
/// them) must produce identical output.
#[test]
fn agents_pane_rebuilds_identically_after_cache_wipe() {
    let temp = tempdir().unwrap();
    let dot_libra = temp.path().join(".libra");
    let sessions_root = dot_libra.join("sessions");
    let store = AgentRunEventStore::new(&sessions_root);
    let thread_id = Uuid::from_u128(0xabcd);

    // A mix of in-flight and terminal runs, persisted out of id order, some with
    // usage and some without, to exercise ordering + the usage join.
    persist_run(
        &store,
        thread_id,
        30,
        AgentRunStatus::Completed,
        Some(RunUsage {
            prompt_tokens: 200,
            completion_tokens: 40,
            cost_estimate_micro_dollars: 9_900,
            ..RunUsage::default()
        }),
    );
    persist_run(&store, thread_id, 31, AgentRunStatus::Running, None);
    persist_run(
        &store,
        thread_id,
        29,
        AgentRunStatus::Failed,
        Some(RunUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            ..RunUsage::default()
        }),
    );

    let first = render_from_disk(&sessions_root);

    // Simulate a projection-cache wipe: create then remove `.libra/cache/`.
    let cache_dir = dot_libra.join("cache");
    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
    std::fs::write(cache_dir.join("stale.bin"), b"stale").expect("seed cache");
    std::fs::remove_dir_all(&cache_dir).expect("wipe cache");

    let rebuilt = render_from_disk(&sessions_root);

    assert_eq!(
        first, rebuilt,
        "the agent pane must rebuild byte-identically from JSONL after a cache wipe",
    );
    // Sanity: the in-flight run (31) sorts before the terminal ones (29/30).
    let running = first.find("running").expect("running row");
    let failed = first.find("failed").expect("failed row");
    let completed = first.find("completed").expect("completed row");
    assert!(
        running < failed && running < completed,
        "in-flight runs must render before terminal ones: {first}",
    );
}

/// An empty sessions root renders the documented placeholder rather than a blank
/// or erroring pane (best-effort read path).
#[test]
fn agents_pane_empty_when_no_runs_persisted() {
    let temp = tempdir().unwrap();
    let sessions_root = temp.path().join(".libra").join("sessions");
    let pane = render_from_disk(&sessions_root);
    assert!(
        pane.contains("No sub-agent runs recorded yet."),
        "empty pane placeholder: {pane}",
    );
}
