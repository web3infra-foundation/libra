//! CEX-S2-14 scheduler-side controlled parallelism.
//!
//! These tests pin the pure admission/queueing model before runtime dispatch
//! consumes it: disjoint write scopes may run together up to `max_parallel`,
//! over-capacity work queues, and overlapping file scopes enter the conflict
//! queue until the blocking run finishes.

use libra::internal::ai::agent_run::{
    AgentRunId, AgentTaskId, ParallelAdmissionConfig, ParallelAdmissionDecision,
    ParallelQueueReason, ParallelRunState, ParallelSchedulerState, ParallelTaskRequest,
};

fn request(scope: &[&str]) -> ParallelTaskRequest {
    ParallelTaskRequest::new(AgentTaskId::new(), AgentRunId::new(), scope.iter().copied())
}

fn assert_spawn_now(decision: ParallelAdmissionDecision) -> AgentRunId {
    match decision {
        ParallelAdmissionDecision::SpawnNow { agent_run_id } => agent_run_id,
        other => panic!("expected SpawnNow, got {other:?}"),
    }
}

#[test]
fn disjoint_file_scopes_spawn_up_to_parallel_limit() {
    let mut state = ParallelSchedulerState::new(ParallelAdmissionConfig::new(2));

    let first = assert_spawn_now(state.admit(request(&["src/worker_a.rs"])));
    let second = assert_spawn_now(state.admit(request(&["docs/worker_b.md"])));

    assert_eq!(state.running_run_ids(), vec![first, second]);
    assert_eq!(state.queued_count(), 0);
    assert_eq!(state.conflict_queued_count(), 0);

    let snapshot = state.snapshot();
    assert_eq!(snapshot.max_parallel, 2);
    assert_eq!(snapshot.running.len(), 2);
    assert!(
        snapshot
            .running
            .iter()
            .all(|run| run.state == ParallelRunState::Running)
    );
}

#[test]
fn zero_max_parallel_config_is_clamped_before_admission() {
    let deserialized: ParallelAdmissionConfig =
        serde_json::from_str(r#"{"max_parallel":0}"#).expect("config JSON must parse");
    assert_eq!(deserialized.max_parallel, 1);

    let mut state = ParallelSchedulerState::new(ParallelAdmissionConfig { max_parallel: 0 });
    assert_eq!(state.config().max_parallel, 1);

    let running = assert_spawn_now(state.admit(request(&["src/a.rs"])));
    assert_eq!(state.running_run_ids(), vec![running]);
}

#[test]
fn exceeding_max_parallel_queues_without_spawning() {
    let mut state = ParallelSchedulerState::new(ParallelAdmissionConfig::new(2));
    assert_spawn_now(state.admit(request(&["src/a.rs"])));
    assert_spawn_now(state.admit(request(&["docs/b.md"])));

    let third = request(&["tests/c.rs"]);
    let third_id = third.agent_run_id;
    match state.admit(third) {
        ParallelAdmissionDecision::Queued {
            agent_run_id,
            reason,
        } => {
            assert_eq!(agent_run_id, third_id);
            assert_eq!(reason, ParallelQueueReason::MaxParallelReached);
        }
        other => panic!("expected max_parallel queueing, got {other:?}"),
    }

    assert_eq!(state.running_count(), 2);
    assert_eq!(state.queued_count(), 1);
    assert_eq!(state.snapshot().queued[0].state, ParallelRunState::Queued);
}

#[test]
fn queued_predecessor_conflict_enters_conflict_queue() {
    let mut state = ParallelSchedulerState::new(ParallelAdmissionConfig::new(1));
    let first = assert_spawn_now(state.admit(request(&["src/a.rs"])));

    let queued = request(&["docs"]);
    let queued_id = queued.agent_run_id;
    assert!(matches!(
        state.admit(queued),
        ParallelAdmissionDecision::Queued {
            reason: ParallelQueueReason::MaxParallelReached,
            ..
        }
    ));

    let blocked = request(&["docs/readme.md"]);
    let blocked_id = blocked.agent_run_id;
    match state.admit(blocked) {
        ParallelAdmissionDecision::ConflictQueued {
            agent_run_id,
            conflicts_with,
        } => {
            assert_eq!(agent_run_id, blocked_id);
            assert_eq!(conflicts_with, vec![queued_id]);
        }
        other => panic!("expected queued-predecessor conflict, got {other:?}"),
    }

    assert_eq!(state.queued_count(), 1);
    assert_eq!(state.conflict_queued_count(), 1);

    assert_eq!(state.finish(first).spawned, vec![queued_id]);
    assert_eq!(state.finish(queued_id).spawned, vec![blocked_id]);
}

#[test]
fn overlapping_file_scope_enters_conflict_queue_and_promotes_after_finish() {
    let mut state = ParallelSchedulerState::new(ParallelAdmissionConfig::new(2));
    let running = assert_spawn_now(state.admit(request(&["src"])));

    let blocked = request(&["src/main.rs"]);
    let blocked_id = blocked.agent_run_id;
    match state.admit(blocked) {
        ParallelAdmissionDecision::ConflictQueued {
            agent_run_id,
            conflicts_with,
        } => {
            assert_eq!(agent_run_id, blocked_id);
            assert_eq!(conflicts_with, vec![running]);
        }
        other => panic!("expected conflict queueing, got {other:?}"),
    }

    assert_eq!(state.running_count(), 1);
    assert_eq!(state.conflict_queued_count(), 1);
    assert_eq!(
        state.snapshot().conflict_queued[0].state,
        ParallelRunState::ConflictQueued,
    );

    let promotions = state.finish(running);
    assert!(promotions.completed_removed);
    assert_eq!(promotions.spawned, vec![blocked_id]);
    assert_eq!(state.running_run_ids(), vec![blocked_id]);
    assert_eq!(state.conflict_queued_count(), 0);
}

#[test]
fn older_conflict_queued_run_promotes_before_later_normal_queue_run() {
    let mut state = ParallelSchedulerState::new(ParallelAdmissionConfig::new(1));
    let blocker = assert_spawn_now(state.admit(request(&["src"])));

    let conflict = request(&["src/main.rs"]);
    let conflict_id = conflict.agent_run_id;
    assert!(matches!(
        state.admit(conflict),
        ParallelAdmissionDecision::ConflictQueued { .. }
    ));

    let normal = request(&["docs/readme.md"]);
    let normal_id = normal.agent_run_id;
    assert!(matches!(
        state.admit(normal),
        ParallelAdmissionDecision::Queued {
            reason: ParallelQueueReason::MaxParallelReached,
            ..
        }
    ));

    assert_eq!(state.finish(blocker).spawned, vec![conflict_id]);
    assert_eq!(state.running_run_ids(), vec![conflict_id]);
    assert_eq!(state.finish(conflict_id).spawned, vec![normal_id]);
}

#[test]
fn path_overlap_is_component_wise_and_root_scope_conflicts_with_everything() {
    let mut disjoint = ParallelSchedulerState::new(ParallelAdmissionConfig::new(2));
    assert_spawn_now(disjoint.admit(request(&["src"])));
    assert_spawn_now(disjoint.admit(request(&["src-generated/output.rs"])));
    assert_eq!(
        disjoint.running_count(),
        2,
        "`src` must not conflict with sibling `src-generated`",
    );

    let mut root = ParallelSchedulerState::new(ParallelAdmissionConfig::new(2));
    let root_run = assert_spawn_now(root.admit(request(&["."])));
    match root.admit(request(&["docs/readme.md"])) {
        ParallelAdmissionDecision::ConflictQueued { conflicts_with, .. } => {
            assert_eq!(conflicts_with, vec![root_run]);
        }
        other => panic!("root scope must conflict with any write scope, got {other:?}"),
    }
}

#[test]
fn queued_runs_promote_in_spawn_order_when_slots_open() {
    let mut state = ParallelSchedulerState::new(ParallelAdmissionConfig::new(1));
    let first = assert_spawn_now(state.admit(request(&["src/a.rs"])));

    let second = request(&["docs/b.md"]);
    let second_id = second.agent_run_id;
    let third = request(&["tests/c.rs"]);
    let third_id = third.agent_run_id;
    assert!(matches!(
        state.admit(second),
        ParallelAdmissionDecision::Queued {
            reason: ParallelQueueReason::MaxParallelReached,
            ..
        }
    ));
    assert!(matches!(
        state.admit(third),
        ParallelAdmissionDecision::Queued {
            reason: ParallelQueueReason::MaxParallelReached,
            ..
        }
    ));

    let first_promotions = state.finish(first);
    assert_eq!(first_promotions.spawned, vec![second_id]);
    let second_promotions = state.finish(second_id);
    assert_eq!(second_promotions.spawned, vec![third_id]);
}
