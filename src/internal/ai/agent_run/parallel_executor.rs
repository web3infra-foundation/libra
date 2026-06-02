//! CEX-S2-14 async parallel executor — drives [`ParallelSchedulerState`].
//!
//! Runs a batch of sub-agent tasks at most `max_parallel` concurrently,
//! serialising file-scope conflicts and promoting queued tasks as running ones
//! finish. The admission *policy* (capacity + conflict) is the pure
//! [`ParallelSchedulerState`]; this module adds the async execution loop on top
//! via a [`FuturesUnordered`].
//!
//! [`run_parallel`] is generic over the per-task `run` operation so the
//! scheduling is unit-testable without the dispatcher or a live provider;
//! production passes a closure that calls
//! `DefaultSubAgentDispatcher::dispatch`. The whole batch runs inside a single
//! future (no `tokio::spawn`), so `run`'s futures may borrow non-`'static`
//! session state — matching how `dispatch` borrows its `DispatchContext`.

use std::{collections::HashMap, future::Future};

use futures::stream::{FuturesUnordered, StreamExt};

use super::{
    AgentRunId, AgentTaskId,
    parallel::{
        ParallelAdmissionConfig, ParallelAdmissionDecision, ParallelSchedulerState,
        ParallelTaskRequest,
    },
};

/// One task in a parallel batch: its scheduler identity, repo-relative write
/// scope (empty = no file-scope conflict), and the opaque `payload` handed to
/// the `run` closure.
pub struct ParallelTask<P> {
    /// The task's id (for scheduler tracking / observability).
    pub task_id: AgentTaskId,
    /// Repo-relative write scope; an overlap with another task's scope routes
    /// the later task into the conflict queue (serialised, never co-running).
    pub write_scope: Vec<String>,
    /// Opaque payload passed to `run` when this task is spawned.
    pub payload: P,
}

/// Drive `tasks` through the scheduler: run at most `config.max_parallel`
/// concurrently, serialise file-scope conflicts, and promote queued tasks as
/// running ones finish. Returns each task's result in the **input order**.
///
/// `run` executes one task's payload. Pure scheduling lives in
/// [`ParallelSchedulerState`]; this is the async driver around it.
pub async fn run_parallel<P, F, Fut, T>(
    tasks: Vec<ParallelTask<P>>,
    config: ParallelAdmissionConfig,
    run: F,
) -> Vec<T>
where
    F: Fn(P) -> Fut,
    Fut: Future<Output = T>,
{
    let mut scheduler = ParallelSchedulerState::new(config);
    let mut payloads: Vec<Option<P>> = Vec::with_capacity(tasks.len());
    let mut results: Vec<Option<T>> = Vec::with_capacity(tasks.len());
    let mut index_by_id: HashMap<AgentRunId, usize> = HashMap::new();
    let futures = FuturesUnordered::new();

    // Single tagged-future factory so every pushed future has one concrete type
    // (`FuturesUnordered` requires it) and carries its run id + input index back
    // out for result placement + scheduler `finish`.
    let run_ref = &run;
    let spawn = move |run_id: AgentRunId, index: usize, payload: P| async move {
        (run_id, index, run_ref(payload).await)
    };

    // Admit every task in input order. The scheduler spawns up to capacity and
    // holds the rest (capacity- or conflict-queued) in its own internal queues.
    for (index, task) in tasks.into_iter().enumerate() {
        let run_id = AgentRunId::new();
        index_by_id.insert(run_id, index);
        payloads.push(Some(task.payload));
        results.push(None);
        let request = ParallelTaskRequest::new(task.task_id, run_id, task.write_scope);
        if let ParallelAdmissionDecision::SpawnNow { agent_run_id } = scheduler.admit(request) {
            let payload = payloads[index].take().expect("a task is spawned once");
            futures.push(spawn(agent_run_id, index, payload));
        }
    }

    // Each completion frees a slot; ask the scheduler which queued tasks may now
    // spawn and push them. `FuturesUnordered::push` during iteration is sound:
    // `next().await`'s borrow ends before the loop body runs.
    let mut futures = futures;
    while let Some((finished_id, index, output)) = futures.next().await {
        results[index] = Some(output);
        for promoted in scheduler.finish(finished_id).spawned {
            let pindex = index_by_id[&promoted];
            let payload = payloads[pindex].take().expect("a task is spawned once");
            futures.push(spawn(promoted, pindex, payload));
        }
    }

    results
        .into_iter()
        .map(|slot| slot.expect("every admitted task ran to completion"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    /// A `run` closure that records peak concurrency: increment on entry, yield
    /// repeatedly (so the executor polls every co-runnable task before any
    /// completes), then decrement. Returns the payload so result ordering is
    /// checkable. `Fn` (clones the Arcs per call) so it satisfies `run_parallel`.
    fn gauge(
        current: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    ) -> impl Fn(usize) -> std::pin::Pin<Box<dyn Future<Output = usize>>> {
        move |payload: usize| {
            let current = Arc::clone(&current);
            let peak = Arc::clone(&peak);
            Box::pin(async move {
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
                current.fetch_sub(1, Ordering::SeqCst);
                payload
            })
        }
    }

    fn task(scope: &[&str], payload: usize) -> ParallelTask<usize> {
        ParallelTask {
            task_id: AgentTaskId::new(),
            write_scope: scope.iter().map(|s| s.to_string()).collect(),
            payload,
        }
    }

    #[tokio::test]
    async fn runs_up_to_max_parallel_concurrently_and_preserves_order() {
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        // Four disjoint (no write_scope) tasks, cap 2.
        let tasks = (0..4).map(|i| task(&[], i)).collect();
        let config = ParallelAdmissionConfig::new(2);
        let out = run_parallel(
            tasks,
            config,
            gauge(Arc::clone(&current), Arc::clone(&peak)),
        )
        .await;

        assert_eq!(out, vec![0, 1, 2, 3], "results returned in input order");
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "exactly max_parallel co-run"
        );
        assert_eq!(current.load(Ordering::SeqCst), 0, "all tasks drained");
    }

    #[tokio::test]
    async fn max_parallel_one_runs_fully_serially() {
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks = (0..3).map(|i| task(&[], i)).collect();
        let out = run_parallel(
            tasks,
            ParallelAdmissionConfig::new(1),
            gauge(Arc::clone(&current), Arc::clone(&peak)),
        )
        .await;
        assert_eq!(out, vec![0, 1, 2]);
        assert_eq!(peak.load(Ordering::SeqCst), 1, "cap 1 never co-runs");
    }

    #[tokio::test]
    async fn conflicting_write_scope_is_serialised_despite_capacity() {
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        // Two tasks touching the SAME path, capacity 2 — the scheduler
        // conflict-queues the second so they never co-run (peak stays 1).
        let tasks = vec![task(&["src/a.rs"], 0), task(&["src/a.rs"], 1)];
        let out = run_parallel(
            tasks,
            ParallelAdmissionConfig::new(2),
            gauge(Arc::clone(&current), Arc::clone(&peak)),
        )
        .await;
        assert_eq!(
            out,
            vec![0, 1],
            "both conflicting tasks still complete, in order"
        );
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "file-scope conflict serialises even under spare capacity",
        );
    }

    #[tokio::test]
    async fn empty_batch_returns_empty() {
        let out = run_parallel(
            Vec::<ParallelTask<usize>>::new(),
            ParallelAdmissionConfig::new(2),
            gauge(Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))),
        )
        .await;
        assert!(out.is_empty());
    }
}
