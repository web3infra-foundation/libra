/**
 * Local view-model types for the workflow pane.
 *
 * These mirror the legacy mock fixture shape so the existing Card / Detail
 * components can stay structurally similar; in Phase 1 they are produced from
 * the live `CodeUiSessionSnapshot` rather than `@/lib/mock`.
 */

export type StepStatus = "queued" | "running" | "done" | "failed";

export type PlanStep = {
  id: string;
  label: string;
  status: StepStatus;
};

export type Plan = {
  id: string;
  steps: PlanStep[];
};

export type ExecutionRunResult = "pass" | "fail" | "running";

export type ExecutionRun = {
  id: string;
  step: string;
  result: ExecutionRunResult;
  ago: string;
  label: string;
  details?: string;
};

export type WorkflowTask = {
  id: string;
  title: string;
  status: StepStatus;
  details?: string;
  ago: string;
};

export type EvidenceKind = "tool" | "frame" | "patch";

export type EvidenceRow = {
  kind: EvidenceKind;
  label: string;
  meta: string;
};

export type IntentDoc = {
  title: string;
  revision: string;
  summary: string;
  constraints: string[];
  confirmed: boolean;
};

export type WorkflowState = {
  currentPhase: number;
  intent: IntentDoc;
  plans: { execution: Plan; test: Plan };
  tasks: WorkflowTask[];
  runs: ExecutionRun[];
  evidence: EvidenceRow[];
};

export type DetailKind =
  | "intent"
  | "plan-step"
  | "task"
  | "run"
  | "validation"
  | "release";

export type DetailState =
  | { kind: "intent"; data: IntentDoc }
  | {
      kind: "plan-step";
      data: { step: PlanStep; planKind: "execution" | "test"; planId: string };
    }
  | { kind: "task"; data: WorkflowTask }
  | { kind: "run"; data: ExecutionRun }
  | { kind: "validation" }
  | { kind: "release" };

/** Empty workflow used as a fallback when the snapshot has no plans yet. */
export const EMPTY_WORKFLOW: WorkflowState = {
  currentPhase: 0,
  intent: {
    title: "No active intent",
    revision: "—",
    summary: "Start a libra code thread to populate this view.",
    constraints: [],
    confirmed: false,
  },
  plans: {
    execution: { id: "—", steps: [] },
    test: { id: "—", steps: [] },
  },
  tasks: [],
  runs: [],
  evidence: [],
};
