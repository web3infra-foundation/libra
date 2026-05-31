"use client";

import { useState } from "react";
import type { ReactNode } from "react";

import { IconX } from "@/components/icons";
import { cn } from "@/lib/utils";

import { Markdown } from "./markdown";
import { statusMeta } from "./status-meta";
import type { CodeUiSessionSnapshot } from "@/lib/code-ui/types";
import type {
  DetailState,
  ExecutionRun,
  IntentDoc,
  PlanStep,
  StepStatus,
  WorkflowTask,
} from "./types";

const MAX_DETAIL_PREVIEW_CHARS = 20 * 1024;

type Props = {
  detail: DetailState | null;
  onClose: () => void;
  snapshot: CodeUiSessionSnapshot | null;
};

export function DetailPanel({ detail, onClose, snapshot }: Props) {
  const open = !!detail;

  return (
    <>
      <div
        onClick={onClose}
        className={cn(
          "absolute inset-0 z-10 transition-opacity duration-[180ms] ease-out",
          open ? "pointer-events-auto opacity-100" : "pointer-events-none opacity-0",
        )}
        style={{ background: "rgba(20,18,14,0.12)" }}
      />
      <div
        className={cn(
          "absolute right-0 top-0 bottom-0 z-20 flex w-[400px] flex-col border-l border-rule-2 bg-paper transition-transform duration-[220ms]",
          open ? "translate-x-0" : "translate-x-[105%]",
        )}
        style={{
          boxShadow: "-12px 0 30px -12px rgba(0,0,0,0.12)",
          transitionTimingFunction: "cubic-bezier(0.22, 0.61, 0.36, 1)",
        }}
      >
        {detail && (
          <DetailContent detail={detail} snapshot={snapshot} onClose={onClose} />
        )}
      </div>
    </>
  );
}

function DetailContent({
  detail,
  snapshot,
  onClose,
}: {
  detail: DetailState;
  snapshot: CodeUiSessionSnapshot | null;
  onClose: () => void;
}) {
  const meta = detailMeta(detail);
  return (
    <>
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-rule px-3.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="mono rounded-sm border border-rule-2 bg-paper-2 px-1 py-px text-[9.5px] font-semibold uppercase tracking-[0.04em] text-ink-3">
            {meta.badge}
          </span>
          <span className="text-[13px] font-semibold">{meta.title}</span>
          {meta.subtitle && (
            <span className="mono text-[10.5px] text-ink-3">{meta.subtitle}</span>
          )}
        </div>
        <button
          type="button"
          onClick={onClose}
          className="grid h-7 w-7 place-items-center rounded-md text-ink-3"
          title="Close"
        >
          <IconX size={14} />
        </button>
      </header>
      <div className="flex-1 overflow-y-auto px-4 pb-7 pt-4">
        {detail.kind === "intent" && <IntentDetail intent={detail.data} />}
        {detail.kind === "plan-step" && <PlanStepDetail data={detail.data} />}
        {detail.kind === "task" && <TaskDetail task={detail.data} />}
        {detail.kind === "run" && <RunDetail run={detail.data} />}
        {detail.kind === "validation" && <ValidationDetail snapshot={snapshot} />}
        {detail.kind === "release" && <ReleaseDetail snapshot={snapshot} />}
      </div>
    </>
  );
}

function detailMeta(d: DetailState) {
  switch (d.kind) {
    case "intent":
      return { badge: "Phase 0", title: "Intent", subtitle: d.data.revision };
    case "plan-step":
      return {
        badge: "Phase 1",
        title: d.data.planKind === "test" ? "Test step" : "Execution step",
        subtitle: d.data.step.id,
      };
    case "task":
      return { badge: "Phase 2", title: "Task", subtitle: d.data.id };
    case "run":
      return { badge: "Phase 2", title: "Run", subtitle: d.data.id };
    case "validation":
      return { badge: "Phase 3", title: "Validation", subtitle: "audit" };
    case "release":
      return { badge: "Phase 4", title: "Release", subtitle: "decision" };
  }
}

function Section({
  label,
  children,
  mono,
}: {
  label: string;
  children: ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="mb-[18px]">
      <div className="mb-2 text-[10px] font-medium uppercase tracking-[0.08em] text-ink-3">
        {label}
      </div>
      <div className={cn(mono && "mono")}>{children}</div>
    </div>
  );
}

function KV({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex border-b border-rule py-[5px] text-[12px]">
      <span className="w-[110px] shrink-0 text-ink-3">{k}</span>
      <span className="mono flex-1 text-[11.5px] text-ink">{v}</span>
    </div>
  );
}

function IntentDetail({ intent }: { intent: IntentDoc }) {
  const lines = [`# ${intent.title}`, "", intent.summary || "No intent summary is available yet."];
  if (intent.constraints.length > 0) {
    lines.push("", "## Constraints", "", ...intent.constraints.map((c) => `- ${c}`));
  }

  return <Markdown source={lines.join("\n")} />;
}

function PlanStepDetail({
  data,
}: {
  data: {
    step: PlanStep;
    planKind: "execution" | "test";
    planId: string;
    planTitle?: string;
    planSummary?: string;
  };
}) {
  const { step, planKind, planId, planTitle, planSummary } = data;
  const meta = statusMeta(step.status);
  return (
    <>
      <div className="mb-1.5 text-[15px] font-semibold tracking-[-0.01em]">
        {step.label}
      </div>
      <div className="mb-[18px]">
        <span
          className="mono rounded-sm px-2 py-px text-[10px] tracking-[0.05em]"
          style={{
            color: meta.color,
            background: `color-mix(in oklch, ${meta.color} 12%, var(--paper))`,
          }}
        >
          {meta.label}
        </span>
      </div>

      <Section label="Metadata">
        <KV k="Step ID" v={step.id} />
        <KV k="Plan" v={planId} />
        <KV k="Plan title" v={planTitle ?? "—"} />
        <KV k="Kind" v={planKind === "test" ? "test" : "execution"} />
        <KV k="Status" v={step.status} />
      </Section>

      {planSummary?.trim() && (
        <Section label="Plan summary">
          <div className="text-[12.5px] leading-[1.55] text-ink-2">
            {planSummary.trim()}
          </div>
        </Section>
      )}

      <Section label="Purpose">
        <div className="text-[12.5px] leading-[1.55] text-ink-2">
          {planKind === "test"
            ? "Verification step — asserts behavior after the execution DAG settles. Failures route back into a new plan revision."
            : "Execution step — mutates cache/code inside the sandbox. Output is captured as an append-only PatchSet bound to the parent plan."}
        </div>
      </Section>

      <Section label="Runtime data">
        <div className="text-[12px] leading-[1.55] text-ink-3">
          This snapshot does not attach tool-call details directly to plan steps yet. Tool output appears under Execution Runs and Terminal.
        </div>
      </Section>

      <Section label="Sibling steps">
        <div className="text-[12px] text-ink-3">
          Linked into the plan DAG. Downstream gates won&apos;t open until this node reports DONE.
        </div>
      </Section>
    </>
  );
}

function TaskDetail({ task }: { task: WorkflowTask }) {
  const meta = statusMeta(task.status);
  return (
    <>
      <div className="mb-3 flex items-center gap-2.5">
        <div className="min-w-0 flex-1 text-[15px] font-semibold">
          {task.title}
        </div>
        <span
          className="mono shrink-0 rounded-sm px-2 py-px text-[10px] tracking-[0.05em]"
          style={{
            color: meta.color,
            background: `color-mix(in oklch, ${meta.color} 12%, var(--paper))`,
          }}
        >
          {meta.label}
        </span>
      </div>

      <Section label="Metadata">
        <KV k="Task ID" v={task.id} />
        <KV k="Status" v={task.status} />
        <KV k="Updated" v={task.ago || "—"} />
      </Section>

      <Section label="Details" mono>
        {task.details ? (
          <ExpandableTextBlock
            text={task.details}
            className="rounded-md border border-rule bg-paper-2 p-3 text-[11px] leading-[1.55] text-ink"
          />
        ) : (
          <div className="text-[12px] leading-[1.55] text-ink-3">
            No task details are attached to this snapshot.
          </div>
        )}
      </Section>
    </>
  );
}

function RunDetail({ run }: { run: ExecutionRun }) {
  const status: StepStatus =
    run.result === "pass"
      ? "done"
      : run.result === "running"
        ? "running"
        : "failed";
  const meta = statusMeta(status);

  return (
    <>
      <div className="mb-3 flex items-center gap-2.5">
        <div className="mono text-[15px] font-semibold">{run.id}</div>
        <span
          className="mono rounded-sm px-2 py-px text-[10px] tracking-[0.05em]"
          style={{
            color: meta.color,
            background: `color-mix(in oklch, ${meta.color} 12%, var(--paper))`,
          }}
        >
          {meta.label}
        </span>
      </div>

      <Section label="Metadata">
        <KV k="Run ID" v={run.id} />
        <KV k="Step" v={run.step} />
        <KV k="Result" v={run.result} />
        <KV k="Summary" v={run.label || "—"} />
        <KV k="Finished" v={run.ago} />
      </Section>

      <Section label="Output" mono>
        {run.details ? (
          <ExpandableTextBlock
            text={run.details}
            className="rounded-md border border-rule bg-paper-2 p-3 text-[11px] leading-[1.55] text-ink"
          />
        ) : (
          <div className="text-[12px] leading-[1.55] text-ink-3">
            No detailed tool output is attached to this run.
          </div>
        )}
      </Section>
    </>
  );
}

function ValidationDetail({ snapshot }: { snapshot: CodeUiSessionSnapshot | null }) {
  const pendingInteractions =
    snapshot?.interactions.filter((interaction) => interaction.status === "pending").length ?? 0;
  const count = (value: number) => (snapshot ? String(value) : "—");
  return (
    <>
      <div className="mb-2.5 text-[15px] font-semibold">Validation gate</div>

      <Section label="Snapshot">
        <KV k="Session status" v={snapshot?.status ?? "—"} />
        <KV k="Plans" v={count(snapshot?.plans.length ?? 0)} />
        <KV k="Tasks" v={count(snapshot?.tasks.length ?? 0)} />
        <KV k="Tool calls" v={count(snapshot?.toolCalls.length ?? 0)} />
        <KV k="PatchSets" v={count(snapshot?.patchsets.length ?? 0)} />
        <KV k="Pending interactions" v={count(pendingInteractions)} />
      </Section>

      <Section label="Controller">
        <KV k="Kind" v={snapshot?.controller.kind ?? "—"} />
        <KV k="Can write" v={snapshot ? (snapshot.controller.canWrite ? "yes" : "no") : "—"} />
        <KV k="Loopback only" v={snapshot ? (snapshot.controller.loopbackOnly ? "yes" : "no") : "—"} />
        <KV k="Owner" v={snapshot?.controller.ownerLabel ?? "—"} />
      </Section>
    </>
  );
}

function ReleaseDetail({ snapshot }: { snapshot: CodeUiSessionSnapshot | null }) {
  const count = (value: number) => (snapshot ? String(value) : "—");
  return (
    <>
      <div className="mb-2.5 text-[15px] font-semibold">Release decision</div>

      <Section label="Session">
        <KV k="Thread" v={snapshot?.threadId ?? "—"} />
        <KV k="Provider" v={snapshot?.provider.provider ?? "—"} />
        <KV k="Model" v={snapshot?.provider.model ?? "—"} />
        <KV k="Working dir" v={snapshot?.workingDir ?? "—"} />
      </Section>

      <Section label="Artifacts">
        <KV k="PatchSets" v={count(snapshot?.patchsets.length ?? 0)} />
        <KV k="Tool calls" v={count(snapshot?.toolCalls.length ?? 0)} />
        <KV k="Tasks" v={count(snapshot?.tasks.length ?? 0)} />
      </Section>

      <Section label="State">
        <KV k="Status" v={snapshot?.status ?? "—"} />
        <KV k="Controller can write" v={snapshot ? (snapshot.controller.canWrite ? "yes" : "no") : "—"} />
        <KV k="Loopback only" v={snapshot ? (snapshot.controller.loopbackOnly ? "yes" : "no") : "—"} />
      </Section>

      <Section label="Output">
        <div className="text-[12px] leading-[1.6] text-ink-3">
          Release details are derived from the live session snapshot.
        </div>
      </Section>
    </>
  );
}

function ExpandableTextBlock({
  text,
  className,
}: {
  text: string;
  className: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const hiddenChars = Math.max(0, text.length - MAX_DETAIL_PREVIEW_CHARS);
  const displayText =
    expanded || hiddenChars === 0 ? text : text.slice(0, MAX_DETAIL_PREVIEW_CHARS);

  return (
    <>
      <pre className={cn("mono m-0 whitespace-pre-wrap break-words", className)}>
        {displayText}
      </pre>
      {hiddenChars > 0 && (
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className="mt-2 rounded-sm border border-rule bg-paper px-1.5 py-0.5 text-[10.5px] font-medium text-ink-3 hover:text-ink"
        >
          {expanded
            ? "Show less"
            : `Show full output (${hiddenChars.toLocaleString()} chars hidden)`}
        </button>
      )}
    </>
  );
}
