"use client";

import {
  IconArrow,
  IconBranch,
  IconClock,
  IconFlask,
  IconPlay,
  IconShield,
  IconSpark,
} from "@/components/icons";
import {
  WORKFLOW,
  type ExecutionRun,
  type IntentDoc,
  type Plan,
  type PlanStep,
} from "@/lib/mock";
import { cn } from "@/lib/utils";

import { Card } from "./card";
import { statusMeta } from "./status-meta";
import type { DetailState } from "./types";

type IntentCardProps = {
  intent: IntentDoc;
  onOpen: (d: DetailState) => void;
  active?: DetailState | null;
};

export function IntentCard({ intent, onOpen, active }: IntentCardProps) {
  const selected = active?.kind === "intent";
  return (
    <Card
      badge="Phase 0"
      title="Intent"
      subtitle={`${intent.revision} · confirmed`}
      icon={<IconSpark size={12} />}
      onClickHead={() => onOpen({ kind: "intent", data: intent })}
      selected={selected}
    >
      <button
        type="button"
        onClick={() => onOpen({ kind: "intent", data: intent })}
        className="block w-full bg-transparent p-0 text-left"
      >
        <div className="mb-1 text-[13px] font-medium">{intent.title}</div>
        <div
          className="overflow-hidden text-[12.5px] leading-[1.5] text-ink-2"
          style={{
            display: "-webkit-box",
            WebkitLineClamp: 2,
            WebkitBoxOrient: "vertical",
          }}
        >
          {intent.summary}
        </div>
        <div className="mono mt-2.5 inline-flex items-center gap-1 text-[11px] text-accent">
          View details <IconArrow size={11} />
        </div>
      </button>
    </Card>
  );
}

type PlanCardProps = {
  phaseBadge: string;
  title: string;
  subtitle: string;
  icon?: React.ReactNode;
  plan: Plan;
  planKind: "execution" | "test";
  active?: boolean;
  gated?: boolean;
  onOpen: (d: DetailState) => void;
  activeDetail: DetailState | null;
};

export function PlanCard({
  phaseBadge,
  title,
  subtitle,
  icon,
  plan,
  planKind,
  active,
  gated,
  onOpen,
  activeDetail,
}: PlanCardProps) {
  return (
    <Card
      badge={phaseBadge}
      title={title}
      subtitle={`${subtitle} · ${plan.steps.length} steps`}
      icon={icon}
      tone={active ? "active" : gated ? "muted" : "default"}
    >
      {gated && (
        <div className="mono mb-2.5 flex items-center gap-1.5 rounded-md bg-paper-2 px-2 py-1.5 text-[11px] text-ink-3">
          <IconClock size={11} /> Stage barrier — runs after execution DAG settles
        </div>
      )}
      <ol className="m-0 list-none p-0">
        {plan.steps.map((step, i) => {
          const selected =
            activeDetail?.kind === "plan-step" &&
            activeDetail.data.step.id === step.id;
          return (
            <StepRow
              key={step.id}
              step={step}
              index={i + 1}
              last={i === plan.steps.length - 1}
              selected={selected}
              onClick={() =>
                onOpen({
                  kind: "plan-step",
                  data: { step, planKind, planId: plan.id },
                })
              }
            />
          );
        })}
      </ol>
    </Card>
  );
}

function StepRow({
  step,
  index,
  last,
  selected,
  onClick,
}: {
  step: PlanStep;
  index: number;
  last: boolean;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <li className="relative flex gap-2.5">
      <div className="relative flex w-[18px] shrink-0 flex-col items-center">
        <div className="mono mt-0.5 grid h-[18px] w-[18px] place-items-center rounded-full border border-rule-2 bg-paper text-[9px] font-semibold text-ink-3">
          {index}
        </div>
        {!last && <div className="mt-0.5 w-px flex-1 bg-rule" />}
      </div>
      <button
        type="button"
        onClick={onClick}
        className={cn(
          "flex-1 rounded-sm text-left transition-colors duration-150",
          selected ? "-ml-1 bg-accent-soft px-1.5 py-1" : "px-0 py-0.5",
          last ? "mb-0" : "mb-1.5",
        )}
      >
        <div className="text-[12.5px] leading-[1.4] text-ink">{step.label}</div>
      </button>
    </li>
  );
}

type RunsCardProps = {
  onOpen: (d: DetailState) => void;
  activeDetail: DetailState | null;
};

export function RunsCard({ onOpen, activeDetail }: RunsCardProps) {
  const execPlan = WORKFLOW.plans.execution;
  const runs = WORKFLOW.runs;
  const runsByStep: Record<string, ExecutionRun[]> = {};
  runs.forEach((r) => {
    (runsByStep[r.step] = runsByStep[r.step] ?? []).push(r);
  });

  return (
    <Card
      badge="Phase 2"
      title="Execution Runs"
      subtitle={`${execPlan.id} · stage-gated DAG`}
      icon={<IconPlay size={12} />}
      tone="active"
    >
      <div className="flex flex-col gap-2.5">
        {execPlan.steps.map((step, i) => (
          <StepRunGroup
            key={step.id}
            step={step}
            index={i + 1}
            runs={runsByStep[step.id] ?? []}
            onOpen={onOpen}
            activeDetail={activeDetail}
          />
        ))}
      </div>
    </Card>
  );
}

function StepRunGroup({
  step,
  runs,
  onOpen,
  activeDetail,
}: {
  step: PlanStep;
  index: number;
  runs: ExecutionRun[];
  onOpen: (d: DetailState) => void;
  activeDetail: DetailState | null;
}) {
  const meta = statusMeta(step.status);
  return (
    <div className="rounded-md border border-rule bg-paper-2 p-2.5">
      <div className="mb-2 flex items-center gap-2">
        <div
          className="grid h-[18px] w-[18px] shrink-0 place-items-center rounded-full border"
          style={{ background: meta.bg, color: meta.color, borderColor: meta.color }}
        >
          {meta.icon}
        </div>
        <span className="mono shrink-0 text-[10.5px] text-ink-3">{step.id}</span>
        <span className="flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[12px] text-ink">
          {step.label}
        </span>
        <span
          className="mono shrink-0 rounded-sm px-1.5 py-px text-[9.5px] tracking-[0.05em]"
          style={{ color: meta.color, background: meta.bg }}
        >
          {meta.label}
        </span>
      </div>
      {runs.length > 0 ? (
        <div className="flex flex-col gap-1 pl-[26px]">
          {runs.map((r) => {
            const selected =
              activeDetail?.kind === "run" && activeDetail.data.id === r.id;
            return (
              <button
                key={r.id}
                type="button"
                onClick={() => onOpen({ kind: "run", data: r })}
                className={cn(
                  "flex cursor-pointer items-center gap-2.5 rounded-md border bg-paper px-2.5 py-1.5 text-left transition-[background,border-color] duration-150",
                  selected ? "border-accent bg-accent-soft" : "border-rule",
                )}
              >
                <span className="mono w-[50px] text-left text-[10.5px] text-ink-3">
                  {r.id}
                </span>
                <span
                  className="mono rounded-sm px-1.5 py-px text-[10px]"
                  style={{
                    background:
                      r.result === "pass"
                        ? "var(--good-soft)"
                        : r.result === "running"
                          ? "var(--accent-soft)"
                          : "var(--bad-soft)",
                    color:
                      r.result === "pass"
                        ? "var(--good)"
                        : r.result === "running"
                          ? "var(--accent)"
                          : "var(--bad)",
                  }}
                >
                  {r.result.toUpperCase()}
                </span>
                <span className="mono ml-auto text-[10.5px] text-ink-2">
                  {r.patch}
                </span>
                <span className="w-[30px] text-right text-[10.5px] text-ink-3">
                  {r.ago}
                </span>
                <IconArrow size={11} className="text-ink-3" />
              </button>
            );
          })}
        </div>
      ) : (
        <div className="pl-[26px] text-[11px] italic text-ink-3">
          no runs yet
        </div>
      )}
    </div>
  );
}

export function ValidationCard({
  onOpen,
  activeDetail,
}: {
  onOpen: (d: DetailState) => void;
  activeDetail: DetailState | null;
}) {
  const selected = activeDetail?.kind === "validation";
  return (
    <Card
      badge="Phase 3"
      title="Validation"
      icon={<IconShield size={12} />}
      tone="muted"
      defaultOpen={false}
      onClickHead={() => onOpen({ kind: "validation" })}
      selected={selected}
    >
      <div className="text-[12px] text-ink-3">
        Waiting for execution DAG to settle, then SAST / SCA / compatibility checks will run and emit Evidence records.
      </div>
    </Card>
  );
}

export function ReleaseCard({
  onOpen,
  activeDetail,
}: {
  onOpen: (d: DetailState) => void;
  activeDetail: DetailState | null;
}) {
  const selected = activeDetail?.kind === "release";
  return (
    <Card
      badge="Phase 4"
      title="Release"
      icon={<IconBranch size={12} />}
      tone="muted"
      defaultOpen={false}
      onClickHead={() => onOpen({ kind: "release" })}
      selected={selected}
    >
      <div className="text-[12px] text-ink-3">
        Low-risk → auto-merge. High-risk → human review. Decision & IntentEvent recorded.
      </div>
    </Card>
  );
}

export { IconFlask };
