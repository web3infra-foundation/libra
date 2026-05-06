/**
 * Derive workflow-pane view-models from the live `CodeUiSessionSnapshot`.
 *
 * Phase 1 ships a low-fidelity mapping: enough so the Workflow / Summary /
 * Diff tabs render real data without leaking the mock module into the
 * production path. Phase 4 expands the mapping with full plan-step status
 * lattice, capability gating, and per-tool detail rendering.
 */

import type {
  CodeUiPlanSnapshot,
  CodeUiPlanStep,
  CodeUiSessionSnapshot,
  CodeUiToolCallSnapshot,
} from "@/lib/code-ui/types";

import type {
  ExecutionRun,
  IntentDoc,
  Plan,
  PlanStep,
  StepStatus,
  WorkflowState,
} from "./types";
import { EMPTY_WORKFLOW } from "./types";

export function deriveWorkflow(
  snapshot: CodeUiSessionSnapshot | null,
): WorkflowState {
  if (!snapshot) return EMPTY_WORKFLOW;
  const intent = deriveIntent(snapshot);
  const [executionPlan, testPlan] = deriveCanonicalPlans(snapshot.plans);
  const runs = deriveRuns(snapshot.toolCalls);
  return {
    currentPhase: deriveCurrentPhase(snapshot),
    intent,
    plans: { execution: executionPlan, test: testPlan },
    runs,
    evidence: [],
  };
}

function deriveIntent(snapshot: CodeUiSessionSnapshot): IntentDoc {
  // The current snapshot doesn't carry a structured IntentSpec; fall back to
  // the most recent user message as a proxy for the active intent.
  const latestUser = [...snapshot.transcript]
    .reverse()
    .find((entry) => entry.kind === "user_message");
  if (!latestUser) {
    return EMPTY_WORKFLOW.intent;
  }
  return {
    title: latestUser.title ?? "Active intent",
    revision: snapshot.threadId ? `thread ${snapshot.threadId.slice(0, 8)}` : "r1",
    summary: latestUser.content ?? "",
    constraints: [],
    confirmed: snapshot.status !== "idle",
  };
}

function deriveCanonicalPlans(plans: CodeUiPlanSnapshot[]): [Plan, Plan] {
  const empty: Plan = { id: "—", steps: [] };
  if (plans.length === 0) return [empty, empty];
  const execution = planFromSnapshot(plans[0]);
  const test = plans.length > 1 ? planFromSnapshot(plans[1]) : empty;
  return [execution, test];
}

function planFromSnapshot(snapshot: CodeUiPlanSnapshot): Plan {
  return {
    id: snapshot.id,
    steps: snapshot.steps.map(stepFromSnapshot),
  };
}

function stepFromSnapshot(step: CodeUiPlanStep, index: number): PlanStep {
  return {
    id: `${step.step}-${index}`,
    label: step.step,
    status: normalizeStepStatus(step.status),
  };
}

function normalizeStepStatus(status: string): StepStatus {
  const lower = status.toLowerCase();
  if (lower === "running" || lower === "in_progress") return "running";
  if (lower === "done" || lower === "completed" || lower === "succeeded") return "done";
  if (lower === "failed" || lower === "error") return "failed";
  return "queued";
}

function deriveRuns(toolCalls: CodeUiToolCallSnapshot[]): ExecutionRun[] {
  return toolCalls.map((tool) => {
    const result =
      tool.status === "succeeded"
        ? "pass"
        : tool.status === "failed"
          ? "fail"
          : "running";
    return {
      id: tool.id,
      step: tool.toolName,
      result,
      ago: relativeAgo(tool.updatedAt),
      patch: tool.summary ?? "",
    };
  });
}

function relativeAgo(updatedAt: string | undefined): string {
  if (!updatedAt) return "";
  const updated = new Date(updatedAt).getTime();
  if (Number.isNaN(updated)) return "";
  const seconds = Math.max(0, Math.floor((Date.now() - updated) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

function deriveCurrentPhase(snapshot: CodeUiSessionSnapshot): number {
  switch (snapshot.status) {
    case "thinking":
      return 0;
    case "awaiting_interaction":
      return 1;
    case "executing_tool":
      return 2;
    case "completed":
      return 4;
    case "error":
      return 3;
    default:
      // No active turn — keep the user on the most-recent meaningful phase.
      return snapshot.toolCalls.length > 0
        ? 2
        : snapshot.plans.length > 0
          ? 1
          : 0;
  }
}
