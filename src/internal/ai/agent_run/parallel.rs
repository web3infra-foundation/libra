//! CEX-S2-14 scheduler-side admission for controlled sub-agent parallelism.
//!
//! CEX-S2-14 调度器侧许可，用于受控子代理并行性。
//!
//! This module is intentionally pure: it decides whether a requested
//! sub-agent run may spawn now, should wait for the `max_parallel` slot, or
//! must enter the file-scope conflict queue. Runtime dispatch still owns
//! provider execution and workspace materialization.

use std::collections::VecDeque;

use serde::{Deserialize, Deserializer, Serialize};

use super::{AgentRunId, AgentTaskId};

/// Admission limits for CEX-S2-14 controlled parallel execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelAdmissionConfig {
    /// Maximum number of non-conflicting runs that may execute at once.
    #[serde(deserialize_with = "deserialize_max_parallel")]
    pub max_parallel: usize,
}

impl ParallelAdmissionConfig {
    pub fn new(max_parallel: usize) -> Self {
        Self {
            max_parallel: max_parallel.max(1),
        }
    }
}

impl Default for ParallelAdmissionConfig {
    fn default() -> Self {
        Self { max_parallel: 2 }
    }
}

/// Request to admit one sub-agent run into the scheduler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParallelTaskRequest {
    pub task_id: AgentTaskId,
    pub agent_run_id: AgentRunId,
    /// Repo-relative write scope. Empty means read-only/no writes, so the
    /// request never conflicts on file scope.
    pub write_scope: Vec<String>,
}

impl ParallelTaskRequest {
    pub fn new(
        task_id: AgentTaskId,
        agent_run_id: AgentRunId,
        write_scope: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            task_id,
            agent_run_id,
            write_scope: write_scope.into_iter().map(Into::into).collect(),
        }
    }
}

/// Why a run was not spawned immediately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ParallelQueueReason {
    MaxParallelReached,
    ConflictingFileScope,
}

/// Result of attempting to admit a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ParallelAdmissionDecision {
    SpawnNow {
        agent_run_id: AgentRunId,
    },
    Queued {
        agent_run_id: AgentRunId,
        reason: ParallelQueueReason,
    },
    ConflictQueued {
        agent_run_id: AgentRunId,
        conflicts_with: Vec<AgentRunId>,
    },
}

/// Runs promoted after a running run completes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelPromotions {
    pub completed_removed: bool,
    pub spawned: Vec<AgentRunId>,
}

/// User/projection-facing status in the parallel scheduler snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ParallelRunState {
    Running,
    Queued,
    ConflictQueued,
}

/// One run row in the serializable scheduler snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelRunSnapshot {
    pub task_id: AgentTaskId,
    pub agent_run_id: AgentRunId,
    pub state: ParallelRunState,
    pub write_scope: Vec<String>,
}

/// Serializable scheduler-side observability state for CEX-S2-14.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelSchedulerSnapshot {
    pub max_parallel: usize,
    pub running: Vec<ParallelRunSnapshot>,
    pub queued: Vec<ParallelRunSnapshot>,
    pub conflict_queued: Vec<ParallelRunSnapshot>,
}

/// Pure scheduler state for controlled parallel admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParallelSchedulerState {
    config: ParallelAdmissionConfig,
    next_sequence: u64,
    running: Vec<TrackedRun>,
    queued: VecDeque<TrackedRun>,
    conflict_queue: VecDeque<TrackedRun>,
}

impl ParallelSchedulerState {
    pub fn new(config: ParallelAdmissionConfig) -> Self {
        Self {
            config: ParallelAdmissionConfig::new(config.max_parallel),
            next_sequence: 0,
            running: Vec::new(),
            queued: VecDeque::new(),
            conflict_queue: VecDeque::new(),
        }
    }

    pub fn config(&self) -> ParallelAdmissionConfig {
        self.config
    }

    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    pub fn queued_count(&self) -> usize {
        self.queued.len()
    }

    pub fn conflict_queued_count(&self) -> usize {
        self.conflict_queue.len()
    }

    pub fn running_run_ids(&self) -> Vec<AgentRunId> {
        self.running.iter().map(|run| run.agent_run_id).collect()
    }

    /// Admit a run according to CEX-S2-14:
    /// - non-overlapping write scopes may run together up to `max_parallel`;
    /// - over-capacity requests enter the normal queue;
    /// - overlapping write scopes enter the conflict queue and do not spawn.
    pub fn admit(&mut self, request: ParallelTaskRequest) -> ParallelAdmissionDecision {
        let run = TrackedRun::from_request(request, self.next_sequence);
        self.next_sequence = self.next_sequence.saturating_add(1);
        let conflicts_with = self.conflicting_runs(&run);
        if !conflicts_with.is_empty() {
            let agent_run_id = run.agent_run_id;
            self.conflict_queue.push_back(run);
            return ParallelAdmissionDecision::ConflictQueued {
                agent_run_id,
                conflicts_with,
            };
        }

        if self.running.len() >= self.config.max_parallel {
            let agent_run_id = run.agent_run_id;
            self.queued.push_back(run);
            return ParallelAdmissionDecision::Queued {
                agent_run_id,
                reason: ParallelQueueReason::MaxParallelReached,
            };
        }

        let agent_run_id = run.agent_run_id;
        self.running.push(run);
        ParallelAdmissionDecision::SpawnNow { agent_run_id }
    }

    /// Mark one running run complete and promote queued work while slots are
    /// available. Returns the promoted run ids in spawn order.
    pub fn finish(&mut self, agent_run_id: AgentRunId) -> ParallelPromotions {
        let before = self.running.len();
        self.running.retain(|run| run.agent_run_id != agent_run_id);
        let completed_removed = before != self.running.len();
        let spawned = self.promote_ready();
        ParallelPromotions {
            completed_removed,
            spawned,
        }
    }

    pub fn snapshot(&self) -> ParallelSchedulerSnapshot {
        ParallelSchedulerSnapshot {
            max_parallel: self.config.max_parallel,
            running: self
                .running
                .iter()
                .map(|run| run.snapshot(ParallelRunState::Running))
                .collect(),
            queued: self
                .queued
                .iter()
                .map(|run| run.snapshot(ParallelRunState::Queued))
                .collect(),
            conflict_queued: self
                .conflict_queue
                .iter()
                .map(|run| run.snapshot(ParallelRunState::ConflictQueued))
                .collect(),
        }
    }

    fn promote_ready(&mut self) -> Vec<AgentRunId> {
        let mut promoted = Vec::new();
        while self.running.len() < self.config.max_parallel {
            if let Some(run) = self.take_next_ready_run() {
                promoted.push(run.agent_run_id);
                self.running.push(run);
                continue;
            }
            break;
        }
        promoted
    }

    fn take_next_ready_run(&mut self) -> Option<TrackedRun> {
        match self.next_ready_location()? {
            PendingQueueLocation::Queued(index) => self.queued.remove(index),
            PendingQueueLocation::ConflictQueued(index) => self.conflict_queue.remove(index),
        }
    }

    fn next_ready_location(&self) -> Option<PendingQueueLocation> {
        let queued = self
            .queued
            .iter()
            .enumerate()
            .filter(|(_, run)| self.is_ready_to_promote(run))
            .map(|(index, run)| (run.sequence, PendingQueueLocation::Queued(index)));

        let conflict_queued = self
            .conflict_queue
            .iter()
            .enumerate()
            .filter(|(_, run)| self.is_ready_to_promote(run))
            .map(|(index, run)| (run.sequence, PendingQueueLocation::ConflictQueued(index)));

        queued
            .chain(conflict_queued)
            .min_by_key(|(sequence, _)| *sequence)
            .map(|(_, location)| location)
    }

    fn is_ready_to_promote(&self, candidate: &TrackedRun) -> bool {
        self.conflicting_running_runs(candidate).is_empty()
            && self.conflicting_older_pending_runs(candidate).is_empty()
    }

    fn conflicting_runs(&self, candidate: &TrackedRun) -> Vec<AgentRunId> {
        self.conflicting_running_runs(candidate)
            .into_iter()
            .chain(self.conflicting_pending_runs(candidate))
            .collect()
    }

    fn conflicting_running_runs(&self, candidate: &TrackedRun) -> Vec<AgentRunId> {
        self.running
            .iter()
            .filter(|run| {
                write_scopes_overlap(
                    &run.normalized_write_scope,
                    &candidate.normalized_write_scope,
                )
            })
            .map(|run| run.agent_run_id)
            .collect()
    }

    fn conflicting_pending_runs(&self, candidate: &TrackedRun) -> Vec<AgentRunId> {
        self.queued
            .iter()
            .chain(self.conflict_queue.iter())
            .filter(|run| {
                write_scopes_overlap(
                    &run.normalized_write_scope,
                    &candidate.normalized_write_scope,
                )
            })
            .map(|run| run.agent_run_id)
            .collect()
    }

    fn conflicting_older_pending_runs(&self, candidate: &TrackedRun) -> Vec<AgentRunId> {
        self.queued
            .iter()
            .chain(self.conflict_queue.iter())
            .filter(|run| run.sequence < candidate.sequence)
            .filter(|run| {
                write_scopes_overlap(
                    &run.normalized_write_scope,
                    &candidate.normalized_write_scope,
                )
            })
            .map(|run| run.agent_run_id)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TrackedRun {
    sequence: u64,
    task_id: AgentTaskId,
    agent_run_id: AgentRunId,
    write_scope: Vec<String>,
    normalized_write_scope: Vec<NormalizedScope>,
}

impl TrackedRun {
    fn from_request(request: ParallelTaskRequest, sequence: u64) -> Self {
        let normalized_write_scope = request
            .write_scope
            .iter()
            .map(|scope| NormalizedScope::from_repo_relative(scope))
            .collect();
        Self {
            sequence,
            task_id: request.task_id,
            agent_run_id: request.agent_run_id,
            write_scope: request.write_scope,
            normalized_write_scope,
        }
    }

    fn snapshot(&self, state: ParallelRunState) -> ParallelRunSnapshot {
        ParallelRunSnapshot {
            task_id: self.task_id,
            agent_run_id: self.agent_run_id,
            state,
            write_scope: self.write_scope.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingQueueLocation {
    Queued(usize),
    ConflictQueued(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NormalizedScope {
    Root,
    Path(Vec<String>),
}

impl NormalizedScope {
    fn from_repo_relative(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "." {
            return Self::Root;
        }
        if is_absolute_like(trimmed) {
            return Self::Root;
        }

        let mut parts = Vec::new();
        for component in trimmed.split(['/', '\\']) {
            match component {
                "" | "." => {}
                ".." => return Self::Root,
                other => parts.push(other.to_string()),
            }
        }

        if parts.is_empty() {
            Self::Root
        } else {
            Self::Path(parts)
        }
    }
}

fn is_absolute_like(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('\\') || path.as_bytes().get(1) == Some(&b':')
}

fn write_scopes_overlap(left: &[NormalizedScope], right: &[NormalizedScope]) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    left.iter().any(|left_scope| {
        right
            .iter()
            .any(|right_scope| scopes_overlap(left_scope, right_scope))
    })
}

fn scopes_overlap(left: &NormalizedScope, right: &NormalizedScope) -> bool {
    match (left, right) {
        (NormalizedScope::Root, _) | (_, NormalizedScope::Root) => true,
        (NormalizedScope::Path(left), NormalizedScope::Path(right)) => {
            left.len() <= right.len() && right[..left.len()] == *left
                || right.len() <= left.len() && left[..right.len()] == *right
        }
    }
}

fn deserialize_max_parallel<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    Ok(value.max(1))
}
