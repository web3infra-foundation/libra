import type { PhaseDescriptor } from "./types";

export const PHASES: PhaseDescriptor[] = [
  { n: 0, key: "intent", label: "Phase 0", name: "Intent", blurb: "Draft & confirm" },
  { n: 1, key: "plan", label: "Phase 1", name: "Plan", blurb: "Analyze & confirm" },
  { n: 2, key: "execution", label: "Phase 2", name: "Execution", blurb: "Stage-gated DAG" },
  { n: 3, key: "validate", label: "Phase 3", name: "Validation", blurb: "Audit & evidence" },
  { n: 4, key: "release", label: "Phase 4", name: "Release", blurb: "Decision" },
];
