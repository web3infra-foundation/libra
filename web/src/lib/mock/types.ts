export type PhaseKey = "intent" | "plan" | "execution" | "validate" | "release";

export type PhaseDescriptor = {
  n: number;
  key: PhaseKey;
  label: string;
  name: string;
  blurb: string;
};

export type Thread = {
  id: string;
  title: string;
  ago: string;
  active?: boolean;
  phase: number;
};

export type ChatRole = "user" | "assistant";

export type ChatMessage = {
  id: string;
  role: ChatRole;
  time: string;
  body: string;
  streaming?: boolean;
};

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
  patch: string;
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
  runs: ExecutionRun[];
  evidence: EvidenceRow[];
};

export type SummaryItem = {
  done: boolean;
  text: string;
};

export type SummaryArtifact = {
  kind: string;
  id: string;
  meta: string;
};

export type SummaryState = {
  progress: SummaryItem[];
  branch: {
    name: string;
    base: string;
    pr: string;
    changes: string;
  };
  artifacts: SummaryArtifact[];
  todo: SummaryItem[];
};

export type DiffLineKind = "ctx" | "add" | "del";

export type DiffLine = {
  kind: DiffLineKind;
  n1?: number;
  n2?: number;
  text: string;
};

export type DiffHunk = {
  header: string;
  lines: DiffLine[];
};

export type DiffFile = {
  path: string;
  add: number;
  del: number;
  hunks: DiffHunk[];
};

export type ReviewState = {
  stats: { files: number; add: number; del: number };
  files: DiffFile[];
};

export type TerminalLineKind =
  | "meta"
  | "prompt"
  | "stdout"
  | "pass"
  | "fail"
  | "run"
  | "info"
  | "warn";

export type TerminalLine = {
  kind: TerminalLineKind;
  text: string;
};
