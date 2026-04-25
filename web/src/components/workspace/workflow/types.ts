import type { ExecutionRun, IntentDoc, PlanStep } from "@/lib/mock";

export type DetailKind =
  | "intent"
  | "plan-step"
  | "run"
  | "validation"
  | "release";

export type DetailState =
  | { kind: "intent"; data: IntentDoc }
  | {
      kind: "plan-step";
      data: { step: PlanStep; planKind: "execution" | "test"; planId: string };
    }
  | { kind: "run"; data: ExecutionRun }
  | { kind: "validation" }
  | { kind: "release" };
