"use client";

import { Fragment, useMemo } from "react";

import { IconBranch } from "@/components/icons";
import { WORKFLOW, type ExecutionRun, type PlanStep } from "@/lib/mock";
import { cn } from "@/lib/utils";

import type { DetailState } from "./types";

type Commit = {
  id: string;
  lane: number;
  hash: string;
  title: string;
  ago: string;
  filled: boolean;
  running?: boolean;
  phase: number;
  parents: string[];
  kind?: "run" | "queued" | "merge";
  runId?: string;
  stepId?: string;
  detailKind?: DetailState["kind"];
  onClick?: (open: (d: DetailState) => void) => void;
  y: number;
};

const ROW_H = 30;
const LANE_W = 16;
const X0 = 18;

type Props = {
  onOpen: (d: DetailState) => void;
  activeDetail: DetailState | null;
};

export function GitTimeline({ onOpen, activeDetail }: Props) {
  const commits = useMemo(() => buildCommits(), []);
  const width = 80;
  const height = commits.length * ROW_H + 12;

  function laneX(lane: number) {
    return X0 + lane * LANE_W;
  }

  return (
    <aside className="flex w-[200px] shrink-0 flex-col border-r border-rule bg-paper-2">
      <div className="flex h-8 shrink-0 items-center gap-1.5 border-b border-rule px-3 text-[11px] font-semibold tracking-[0.02em] text-ink-2">
        <IconBranch size={12} />
        <span>Thread graph</span>
      </div>
      <div className="relative flex-1 overflow-y-auto pb-4">
        <svg width={width} height={height} className="block">
          {commits.map((c, i) => {
            if (!c.parents) return null;
            return c.parents.map((p, k) => {
              const parent = commits.find((x) => x.id === p);
              if (!parent) return null;
              const y1 = parent.y;
              const y2 = c.y;
              const x1 = laneX(parent.lane);
              const x2 = laneX(c.lane);
              if (x1 === x2) {
                return (
                  <line
                    key={`${c.id}-${k}-${i}`}
                    x1={x1}
                    y1={y1}
                    x2={x2}
                    y2={y2}
                    stroke={c.lane === 0 ? "var(--ink-2)" : "var(--accent)"}
                    strokeWidth="1.5"
                  />
                );
              }
              const midY = (y1 + y2) / 2;
              const d = `M ${x1} ${y1} C ${x1} ${midY}, ${x2} ${midY}, ${x2} ${y2}`;
              return (
                <path
                  key={`${c.id}-${k}-${i}`}
                  d={d}
                  fill="none"
                  stroke={c.kind === "merge" ? "var(--ink-2)" : "var(--accent)"}
                  strokeWidth="1.5"
                />
              );
            });
          })}

          {commits.map((c) => {
            const cx = laneX(c.lane);
            const cy = c.y;
            const col = nodeColor(c);
            const active = isNodeActive(c, activeDetail);
            return (
              <g
                key={c.id}
                onClick={() => c.onClick?.(onOpen)}
                style={{ cursor: c.onClick ? "pointer" : "default" }}
              >
                {active && (
                  <circle
                    cx={cx}
                    cy={cy}
                    r="8"
                    fill="none"
                    stroke="var(--accent)"
                    strokeWidth="1.5"
                    opacity="0.5"
                  />
                )}
                <circle
                  cx={cx}
                  cy={cy}
                  r="5"
                  fill={c.filled ? col : "var(--paper)"}
                  stroke={col}
                  strokeWidth="1.5"
                />
                {c.running && (
                  <circle cx={cx} cy={cy} r="2" fill="var(--paper)" />
                )}
              </g>
            );
          })}
        </svg>

        <div className="absolute inset-0">
          {commits.map((c) => (
            <Fragment key={c.id}>
              <button
                type="button"
                onClick={() => c.onClick?.(onOpen)}
                title={c.title}
                className={cn(
                  "absolute m-0 flex max-w-[130px] flex-col gap-0 rounded-sm border-none bg-transparent p-1 text-[11px]",
                  c.onClick
                    ? "cursor-pointer opacity-100"
                    : "cursor-default opacity-75",
                )}
                style={{
                  top: c.y - 7,
                  left: laneX(c.lane) + 10,
                  color: isNodeActive(c, activeDetail)
                    ? "var(--accent)"
                    : "var(--ink)",
                }}
              >
                <span className="block max-w-[128px] overflow-hidden text-ellipsis whitespace-nowrap text-[10.5px] leading-[1.2]">
                  {c.title}
                </span>
              </button>
            </Fragment>
          ))}
        </div>
      </div>
      <div className="mono shrink-0 border-t border-rule bg-paper-2 px-3 py-2 text-[10px]">
        <span className="text-accent">HEAD</span>
        <span className="ml-1 text-ink-3">→ agent/optimistic-mutate</span>
      </div>
    </aside>
  );
}

function nodeColor(c: Commit) {
  if (c.phase === 2) return "var(--accent)";
  if (c.lane > 0) return "var(--accent)";
  if (c.kind === "queued") return "var(--ink-3)";
  return "var(--ink)";
}

function isNodeActive(c: Commit, detail: DetailState | null) {
  if (!detail) return false;
  if (c.detailKind === detail.kind) {
    if (detail.kind === "run") return c.runId === detail.data.id;
    if (detail.kind === "plan-step") return c.stepId === detail.data.step.id;
    return true;
  }
  return false;
}

function buildCommits(): Commit[] {
  let y = 14;
  const rows: Commit[] = [];

  function push(o: Omit<Commit, "y">): Commit {
    const row: Commit = { ...o, y };
    rows.push(row);
    y += ROW_H;
    return row;
  }

  // Phase 0: intent
  push({
    id: "c0",
    lane: 0,
    hash: "a81f",
    title: "intent: confirm",
    ago: "10:44",
    filled: true,
    phase: 0,
    parents: [],
    detailKind: "intent",
    onClick: (open) => open({ kind: "intent", data: WORKFLOW.intent }),
  });

  // Phase 1: plan
  push({
    id: "c1",
    lane: 0,
    hash: "b3d2",
    title: "plan: exec + test",
    ago: "10:45",
    filled: true,
    phase: 1,
    parents: ["c0"],
  });

  // Phase 2: fork
  const fork = push({
    id: "c2",
    lane: 0,
    hash: "c902",
    title: "phase 2: start",
    ago: "10:46",
    filled: true,
    phase: 2,
    parents: ["c1"],
  });

  const execPlan = WORKFLOW.plans.execution;
  const runs: ExecutionRun[] = WORKFLOW.runs;
  let prevRunId = fork.id;
  let firstRun = true;

  runs.forEach((r, i) => {
    const step = execPlan.steps.find((s) => s.id === r.step);
    const running = r.result === "running";
    const c = push({
      id: `r${i}`,
      lane: 1,
      hash: shortHash(r.id),
      title: step ? step.label : r.step,
      ago: r.ago,
      filled: !running,
      running,
      phase: 2,
      kind: running ? undefined : "run",
      parents: [prevRunId],
      runId: r.id,
      stepId: r.step,
      detailKind: "run",
      onClick: (open) => open({ kind: "run", data: r }),
    });
    if (firstRun) {
      c.parents = [fork.id];
      firstRun = false;
    }
    prevRunId = c.id;
  });

  const doneStepIds = new Set(runs.map((r) => r.step));
  execPlan.steps
    .filter((s: PlanStep) => !doneStepIds.has(s.id) && s.status !== "running")
    .forEach((s, i) => {
      const c = push({
        id: `q${i}`,
        lane: 1,
        hash: "····",
        title: s.label,
        ago: "queued",
        filled: false,
        phase: 2,
        kind: "queued",
        parents: [prevRunId],
        stepId: s.id,
        detailKind: "plan-step",
        onClick: (open) =>
          open({
            kind: "plan-step",
            data: { step: s, planKind: "execution", planId: execPlan.id },
          }),
      });
      prevRunId = c.id;
    });

  push({
    id: "c3",
    lane: 0,
    hash: "····",
    title: "validation",
    ago: "pending",
    filled: false,
    phase: 3,
    kind: "queued",
    parents: [fork.id],
    detailKind: "validation",
    onClick: (open) => open({ kind: "validation" }),
  });

  push({
    id: "c4",
    lane: 0,
    hash: "····",
    title: "release",
    ago: "pending",
    filled: false,
    phase: 4,
    kind: "queued",
    parents: ["c3"],
    detailKind: "release",
    onClick: (open) => open({ kind: "release" }),
  });

  return rows;
}

function shortHash(id: string) {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return h.toString(16).slice(0, 4);
}
