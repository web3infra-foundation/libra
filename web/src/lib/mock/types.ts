/**
 * Mock data type definitions used across the workspace UI.
 *
 * These describe the shape of the demo content shipped in `/lib/mock/*` and
 * mirror the wire types the production version will receive from Libra's
 * Rust backend (over the websocket / SSE transport defined in `internal/ai`).
 *
 * No runtime values live here — only `type` declarations — so this module is
 * safe to import from anywhere in the bundle.
 */

/** The five phases of the Libra agent pipeline. Stable, used as a discriminator. */
export type PhaseKey = "intent" | "plan" | "execution" | "validate" | "release";

/** Human-readable descriptor for a phase shown in the {@link PhaseStrip}. */
export type PhaseDescriptor = {
  /** Zero-based phase index, also used to compare against `WorkflowState.currentPhase`. */
  n: number;
  key: PhaseKey;
  /** Short label like "Phase 0". */
  label: string;
  /** Title rendered under the phase chip (e.g. "Intent"). */
  name: string;
  /** One-line tagline shown beneath the title. */
  blurb: string;
};

/** Sidebar row representing a saved conversation/thread. */
export type Thread = {
  id: string;
  title: string;
  /** Relative timestamp string ("1m", "2d") — backend formats these. */
  ago: string;
  /** Whether this thread is the currently selected one on initial load. */
  active?: boolean;
  /** Current phase index used to compute the phase chip. */
  phase: number;
};

/** Author of a chat message. */
export type ChatRole = "user" | "assistant";

/** A single chat message rendered in the conversation pane. */
export type ChatMessage = {
  id: string;
  role: ChatRole;
  /** Wall-clock string formatted as `HH:MM` for display only. */
  time: string;
  /** Plain text body. Newlines are preserved when rendered. */
  body: string;
  /** When true, the assistant message is being streamed character-by-character. */
  streaming?: boolean;
};

/** Lifecycle state of a single plan step. */
export type StepStatus = "queued" | "running" | "done" | "failed";

/** One node in a plan DAG. */
export type PlanStep = {
  id: string;
  label: string;
  status: StepStatus;
};

/** A plan: an ordered list of steps with a stable identifier. */
export type Plan = {
  id: string;
  steps: PlanStep[];
};

/** Outcome of an execution run captured by the agent sandbox. */
export type ExecutionRunResult = "pass" | "fail" | "running";

/** A run of an execution step (i.e., one attempt of a step inside the sandbox). */
export type ExecutionRun = {
  id: string;
  /** The plan-step id this run corresponds to. */
  step: string;
  result: ExecutionRunResult;
  /** Relative finish time. */
  ago: string;
  /** Patch summary string like `+12 −0`. Used as a compact stat. */
  patch: string;
};

/** Evidence categories captured during a workflow. */
export type EvidenceKind = "tool" | "frame" | "patch";

/** A single evidence record (tool invocation, captured frame, patch set). */
export type EvidenceRow = {
  kind: EvidenceKind;
  label: string;
  meta: string;
};

/** An intent document — the user-confirmed goal driving Phase 0/1. */
export type IntentDoc = {
  title: string;
  /** Revision marker (`r1`, `r2`, …). */
  revision: string;
  summary: string;
  /** Hard constraints that must hold across the implementation. */
  constraints: string[];
  /** Whether the user has confirmed the intent. */
  confirmed: boolean;
};

/** Combined workflow state used by the right-hand pipeline view. */
export type WorkflowState = {
  /** Current phase index. */
  currentPhase: number;
  intent: IntentDoc;
  /** The two plans produced in Phase 1 — execution and test. */
  plans: { execution: Plan; test: Plan };
  runs: ExecutionRun[];
  evidence: EvidenceRow[];
};

/** Single line in the Summary "progress" or "to-do" lists. */
export type SummaryItem = {
  done: boolean;
  text: string;
};

/** Captured artifact (PatchSet, Frame, etc.) to surface on the Summary tab. */
export type SummaryArtifact = {
  kind: string;
  id: string;
  meta: string;
};

/** Aggregate state rendered by the Summary tab of the workflow pane. */
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

/** Discriminator for a diff line: context, addition, or deletion. */
export type DiffLineKind = "ctx" | "add" | "del";

/** Single line within a unified diff hunk. */
export type DiffLine = {
  kind: DiffLineKind;
  /** Old-file line number (omitted for pure additions). */
  n1?: number;
  /** New-file line number (omitted for pure deletions). */
  n2?: number;
  text: string;
};

/** A unified diff hunk with its `@@` header. */
export type DiffHunk = {
  header: string;
  lines: DiffLine[];
};

/** Per-file diff entry. */
export type DiffFile = {
  path: string;
  /** Number of additions — pre-aggregated for the file header. */
  add: number;
  /** Number of deletions. */
  del: number;
  hunks: DiffHunk[];
};

/** Aggregate state rendered by the Diff tab of the workflow pane. */
export type ReviewState = {
  stats: { files: number; add: number; del: number };
  files: DiffFile[];
};

/**
 * Categories of lines surfaced in the embedded terminal.
 *
 * - `meta`   – sandbox/runtime banner lines.
 * - `prompt` – user-entered shell command (rendered with a `$` marker).
 * - `stdout` – generic command output.
 * - `pass`   – test pass / success line.
 * - `fail`   – test failure / error line.
 * - `run`    – currently-running test/process.
 * - `info`   – informational message from the agent supervisor.
 * - `warn`   – warning from the agent or compiler.
 */
export type TerminalLineKind =
  | "meta"
  | "prompt"
  | "stdout"
  | "pass"
  | "fail"
  | "run"
  | "info"
  | "warn";

/** Single line in the embedded terminal. */
export type TerminalLine = {
  kind: TerminalLineKind;
  text: string;
};
