use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use uuid::Uuid;

#[derive(Debug)]
struct BaselineMeasurement {
    name: &'static str,
    elapsed: Duration,
}

fn measure(name: &'static str, work: impl FnOnce()) -> BaselineMeasurement {
    let started = Instant::now();
    work();
    BaselineMeasurement {
        name,
        elapsed: started.elapsed(),
    }
}

fn build_100_task_dag() {
    let tasks = (0..100).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
    let mut edges: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for window in tasks.windows(2) {
        edges.entry(window[1]).or_default().push(window[0]);
    }
    assert_eq!(edges.len(), 99);
}

fn targeted_rebuild_10k_events() {
    let thread_id = Uuid::new_v4();
    let events = (0..10_000)
        .map(|ordinal| (thread_id, ordinal, Uuid::new_v4()))
        .collect::<Vec<_>>();
    let materialized = events
        .iter()
        .filter(|(event_thread_id, _, _)| *event_thread_id == thread_id)
        .count();
    assert_eq!(materialized, 10_000);
}

fn compact_live_context_window() {
    let frames = (0..64)
        .map(|index| format!("frame-{index}: {}", "x".repeat(128)))
        .collect::<VecDeque<_>>();
    let summary = frames
        .iter()
        .take(16)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!summary.is_empty());
}

fn audit_flush_and_query() {
    let run_id = Uuid::new_v4();
    let audit = (0..512)
        .map(|ordinal| (run_id, ordinal, format!("event-{ordinal}")))
        .collect::<Vec<_>>();
    let matched = audit
        .iter()
        .filter(|(event_run_id, _, _)| *event_run_id == run_id)
        .count();
    assert_eq!(matched, 512);
}

#[test]
fn ai_runtime_baseline_harness_smoke() {
    let measurements = [
        measure("100_task_dag_build", build_100_task_dag),
        measure("10k_event_targeted_rebuild", targeted_rebuild_10k_events),
        measure("live_context_compaction", compact_live_context_window),
        measure("audit_flush_query", audit_flush_and_query),
    ];

    for measurement in measurements {
        eprintln!("{}: {:?}", measurement.name, measurement.elapsed);
        assert!(
            measurement.elapsed < Duration::from_secs(5),
            "{} baseline smoke exceeded 5s: {:?}",
            measurement.name,
            measurement.elapsed
        );
    }
}
