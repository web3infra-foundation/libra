/**
 * Static workflow demo fixture.
 *
 * Matches the {@link WorkflowState} contract; the pipeline pane, git timeline,
 * detail panel, and summary all read from this single source so the demo is
 * internally consistent (e.g. `runs[*].step` ids must exist in
 * `plans.execution.steps[*].id`).
 *
 * Replace this with a websocket-fed store once the Rust backend exposes the
 * thread state stream.
 */
import type { WorkflowState } from "./types";

export const WORKFLOW: WorkflowState = {
  currentPhase: 2,
  intent: {
    title: "Add optimistic updates to useMutation",
    revision: "r2",
    summary:
      "Introduce optimistic cache patching with rollback-on-error and reconciliation when the server response lands. Preserve ordering across concurrent mutations.",
    constraints: [
      "Do not break MutationOptions<T> public shape",
      "Keep rollback safe under concurrent mutations",
      "Cover happy + error path with tests",
    ],
    confirmed: true,
  },
  plans: {
    execution: {
      id: "plan-exec-04",
      steps: [
        { id: "s1", label: "Snapshot cache at mutate() entry", status: "done" },
        { id: "s2", label: "Apply optimistic patch to subscribers", status: "done" },
        { id: "s3", label: "Per-key revision counter for safe rollback", status: "running" },
        { id: "s4", label: "Reconcile server response into cache", status: "queued" },
        { id: "s5", label: "Surface onError with rollback context", status: "queued" },
      ],
    },
    test: {
      id: "plan-test-02",
      steps: [
        { id: "t1", label: "Happy-path optimistic update reflects immediately", status: "queued" },
        { id: "t2", label: "Failure rolls back and preserves concurrent writes", status: "queued" },
        { id: "t3", label: "Reconciliation replaces optimistic entry", status: "queued" },
      ],
    },
  },
  runs: [
    { id: "run-11", step: "s1", result: "pass", ago: "2m", patch: "+12 −0" },
    { id: "run-12", step: "s2", result: "pass", ago: "2m", patch: "+34 −7" },
    { id: "run-13", step: "s3", result: "running", ago: "now", patch: "…" },
  ],
  evidence: [
    { kind: "tool", label: "read src/lib/query.ts", meta: "214 lines" },
    { kind: "tool", label: "read src/hooks/useMutation.ts", meta: "88 lines" },
    { kind: "tool", label: 'grep "MutationOptions"', meta: "9 matches in 4 files" },
    { kind: "frame", label: "ContextFrame cf-0418", meta: "cache shape captured" },
    { kind: "patch", label: "PatchSet ps-07", meta: "+46 −7 across 2 files" },
  ],
};
