"use client";

import { useState, type ReactNode } from "react";

import {
  IconCheck,
  IconDiff,
  IconFlask,
  IconGit,
  IconPlay,
  IconSpark,
  IconTokens,
} from "@/components/icons";
import { WORKFLOW } from "@/lib/mock";
import { cn } from "@/lib/utils";

import {
  IntentCard,
  PlanCard,
  ReleaseCard,
  RunsCard,
  ValidationCard,
} from "./cards";
import { DetailPanel } from "./detail-panel";
import { GitTimeline } from "./git-timeline";
import { PhaseStrip } from "./phase-strip";
import { ReviewView } from "./review-view";
import { SummaryView } from "./summary-view";
import type { DetailState } from "./types";

type Tab = "pipeline" | "summary" | "diff";

type Props = {
  width: number;
};

export function Workflow({ width }: Props) {
  const [tab, setTab] = useState<Tab>("pipeline");
  const [detail, setDetail] = useState<DetailState | null>(null);

  return (
    <section
      className="relative flex shrink-0 min-w-0 flex-col overflow-hidden border-l border-rule bg-paper"
      style={{ width }}
    >
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-rule pl-4 pr-3.5">
        <div className="flex gap-0.5">
          <TabBtn active={tab === "pipeline"} onClick={() => setTab("pipeline")}>
            <IconGit size={13} /> Workflow
          </TabBtn>
          <TabBtn active={tab === "summary"} onClick={() => setTab("summary")}>
            <IconCheck size={13} /> Summary
          </TabBtn>
          <TabBtn active={tab === "diff"} onClick={() => setTab("diff")}>
            <IconDiff size={13} /> Diff
          </TabBtn>
        </div>
        <div className="flex items-center gap-1.5 text-ink-3">
          <span
            title="Tokens consumed in this thread"
            className="inline-flex items-center gap-1.5 rounded-sm border border-rule-2 bg-paper-2 px-2 py-1 text-[11px] text-ink-2"
          >
            <IconTokens size={11} />
            <span className="mono">48.2k</span>
            <span className="text-[10px] tracking-[0.04em] text-ink-3">
              Token
            </span>
          </span>
        </div>
      </header>

      <div className="flex min-h-0 flex-1 overflow-hidden">
        {tab === "pipeline" && (
          <GitTimeline onOpen={setDetail} activeDetail={detail} />
        )}
        <div className="flex-1 overflow-y-auto px-4 pb-2 pt-3.5">
          {tab === "pipeline" && (
            <PipelineView onOpen={setDetail} activeDetail={detail} />
          )}
          {tab === "summary" && <SummaryView />}
          {tab === "diff" && <ReviewView />}
        </div>
      </div>

      <footer className="flex h-11 shrink-0 items-center justify-between border-t border-rule px-3.5">
        <div className="text-[11px] text-ink-3">
          <span className="mono">thread-t1</span> · 5 events · 2 PatchSets
        </div>
        <div className="flex gap-1.5">
          <button
            type="button"
            className="rounded-md border border-rule-2 bg-paper px-2.5 py-1 text-[11.5px] text-ink-2"
          >
            Pause
          </button>
          <button
            type="button"
            className="inline-flex items-center gap-1.5 rounded-md bg-ink px-2.5 py-1 text-[11.5px] font-medium text-paper"
          >
            <IconPlay size={11} /> Continue
          </button>
        </div>
      </footer>

      <DetailPanel detail={detail} onClose={() => setDetail(null)} />
    </section>
  );
}

function TabBtn({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "-mb-px flex items-center gap-1.5 border-b-[1.5px] px-2.5 py-1.5 text-[12px] font-medium",
        active ? "border-ink text-ink" : "border-transparent text-ink-3",
      )}
    >
      {children}
    </button>
  );
}

function PipelineView({
  onOpen,
  activeDetail,
}: {
  onOpen: (d: DetailState) => void;
  activeDetail: DetailState | null;
}) {
  return (
    <div>
      <PhaseStrip current={WORKFLOW.currentPhase} />
      <IntentCard
        intent={WORKFLOW.intent}
        onOpen={onOpen}
        active={activeDetail}
      />
      <PlanCard
        phaseBadge="Phase 1 · Exec"
        title="Execution Plan"
        subtitle={WORKFLOW.plans.execution.id}
        icon={<IconSpark size={12} />}
        plan={WORKFLOW.plans.execution}
        planKind="execution"
        active
        onOpen={onOpen}
        activeDetail={activeDetail}
      />
      <PlanCard
        phaseBadge="Phase 1 · Test"
        title="Test Plan"
        subtitle={WORKFLOW.plans.test.id}
        icon={<IconFlask size={12} />}
        plan={WORKFLOW.plans.test}
        planKind="test"
        gated
        onOpen={onOpen}
        activeDetail={activeDetail}
      />
      <RunsCard onOpen={onOpen} activeDetail={activeDetail} />
      <ValidationCard onOpen={onOpen} activeDetail={activeDetail} />
      <ReleaseCard onOpen={onOpen} activeDetail={activeDetail} />
    </div>
  );
}
