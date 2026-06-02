"use client";

import { useMemo, useState, type ReactNode } from "react";

import {
  IconCheck,
  IconDiff,
  IconFlask,
  IconGit,
  IconPlay,
  IconSettings,
  IconSpark,
  IconTokens,
} from "@/components/icons";
import { useBrowserController } from "@/lib/code-ui/controller";
import { useCodeUiStore } from "@/lib/code-ui/store";
import { deriveWorkflowSummary } from "@/lib/code-ui/view-model";
import { cn } from "@/lib/utils";

import {
  IntentCard,
  PlanCard,
  ReleaseCard,
  RunsCard,
  TasksCard,
  ValidationCard,
} from "./cards";
import { deriveWorkflow } from "./derive";
import { DetailPanel } from "./detail-panel";
import { GitTimeline } from "./git-timeline";
import { PhaseStrip } from "./phase-strip";
import { ReviewView } from "./review-view";
import { SettingsView } from "./settings-view";
import { SummaryView } from "./summary-view";
import type { DetailState, WorkflowState } from "./types";

type Tab = "pipeline" | "summary" | "diff" | "settings";

type Props = {
  width: number;
};

export function Workflow({ width }: Props) {
  const { snapshot, status } = useCodeUiStore();
  const controller = useBrowserController();
  const [tab, setTab] = useState<Tab>("pipeline");
  const [detail, setDetail] = useState<DetailState | null>(null);

  const workflow = useMemo<WorkflowState>(() => deriveWorkflow(snapshot), [snapshot]);
  const summary = useMemo(() => deriveWorkflowSummary(snapshot), [snapshot]);
  const branchLabel =
    status?.head.type === "branch"
      ? status.head.name
      : status
        ? `detached @ ${status.head.oid.slice(0, 7)}`
        : "—";

  const canCancel =
    !!snapshot &&
    snapshot.controller.canWrite &&
    ["thinking", "executing_tool", "awaiting_interaction"].includes(snapshot.status);
  const canContinue =
    !!snapshot &&
    snapshot.controller.canWrite &&
    snapshot.status === "awaiting_interaction" &&
    snapshot.capabilities.interactiveApprovals;

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
          <TabBtn active={tab === "settings"} onClick={() => setTab("settings")}>
            <IconSettings size={13} /> Settings
          </TabBtn>
        </div>
        <div className="flex items-center gap-1.5 text-ink-3">
          <span
            title="No token usage data yet — wire up in Phase 4."
            className="inline-flex items-center gap-1.5 rounded-sm border border-rule-2 bg-paper-2 px-2 py-1 text-[11px] text-ink-2"
          >
            <IconTokens size={11} />
            <span className="mono">—</span>
            <span className="text-[10px] tracking-[0.04em] text-ink-3">Token</span>
          </span>
        </div>
      </header>

      <div className="flex min-h-0 flex-1 overflow-hidden">
        {tab === "pipeline" && (
          <GitTimeline
            onOpen={setDetail}
            activeDetail={detail}
            workflow={workflow}
            branchLabel={branchLabel}
          />
        )}
        <div className="flex-1 overflow-y-auto px-4 pb-2 pt-3.5">
          {tab === "pipeline" && (
            <PipelineView
              onOpen={setDetail}
              activeDetail={detail}
              workflow={workflow}
            />
          )}
          {tab === "summary" && <SummaryView />}
          {tab === "diff" && <ReviewView />}
          {tab === "settings" && <SettingsView />}
        </div>
      </div>

      <footer className="flex h-11 shrink-0 items-center justify-between border-t border-rule px-3.5">
        <div className="text-[11px] text-ink-3">
          {snapshot?.threadId ? (
            <span className="mono">{snapshot.threadId}</span>
          ) : (
            <span className="italic">no active thread</span>
          )}
          {summary.toolCallCount > 0 && (
            <>
              {" · "}
              <span>{summary.toolCallCount} tool calls</span>
            </>
          )}
          {summary.patchsetCount > 0 && (
            <>
              {" · "}
              <span>{summary.patchsetCount} PatchSets</span>
            </>
          )}
          {summary.pendingInteractions > 0 && (
            <>
              {" · "}
              <span className="text-accent">
                {summary.pendingInteractions} pending interaction{summary.pendingInteractions === 1 ? "" : "s"}
              </span>
            </>
          )}
        </div>
        <div className="flex gap-1.5">
          <button
            type="button"
            disabled={!canCancel}
            onClick={() => {
              if (!canCancel) return;
              void controller.cancel().catch(() => undefined);
            }}
            title={
              canCancel
                ? "Cancel the active turn (Esc-equivalent)"
                : "No active turn to cancel"
            }
            className={cn(
              "rounded-md border px-2.5 py-1 text-[11.5px]",
              canCancel
                ? "border-bad/40 bg-paper text-bad hover:bg-paper-2"
                : "border-rule bg-paper-2 text-ink-3",
            )}
          >
            Cancel
          </button>
          <button
            type="button"
            disabled={!canContinue}
            onClick={() => {
              if (!canContinue) return;
              // Scroll the chat-pane InteractionPanel into view rather than
              // duplicating its controls in the workflow footer.
              const panel = document.getElementById("libra-interaction-panel");
              panel?.scrollIntoView({ behavior: "smooth", block: "center" });
              const focusable = panel?.querySelector<HTMLElement>(
                "button:not([disabled]), input:not([disabled])",
              );
              focusable?.focus();
            }}
            title={
              canContinue
                ? "Jump to the pending interaction in the chat panel"
                : "Continue activates only while waiting on an interaction"
            }
            className={cn(
              "inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-[11.5px] font-medium",
              canContinue
                ? "border-accent-line bg-paper text-accent hover:bg-accent-soft"
                : "border-rule bg-paper-2 text-ink-3",
            )}
          >
            <IconPlay size={11} /> Continue
          </button>
        </div>
      </footer>

      <DetailPanel detail={detail} onClose={() => setDetail(null)} snapshot={snapshot} />
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
  workflow,
}: {
  onOpen: (d: DetailState) => void;
  activeDetail: DetailState | null;
  workflow: WorkflowState;
}) {
  return (
    <div>
      <PhaseStrip current={workflow.currentPhase} />
      <IntentCard intent={workflow.intent} onOpen={onOpen} active={activeDetail} />
      <PlanCard
        phaseBadge="Phase 1 · Exec"
        title="Execution Plan"
        subtitle={workflow.plans.execution.id}
        icon={<IconSpark size={12} />}
        plan={workflow.plans.execution}
        planKind="execution"
        active
        onOpen={onOpen}
        activeDetail={activeDetail}
      />
      <PlanCard
        phaseBadge="Phase 1 · Test"
        title="Test Plan"
        subtitle={workflow.plans.test.id}
        icon={<IconFlask size={12} />}
        plan={workflow.plans.test}
        planKind="test"
        gated
        onOpen={onOpen}
        activeDetail={activeDetail}
      />
      <TasksCard
        tasks={workflow.tasks}
        onOpen={onOpen}
        activeDetail={activeDetail}
      />
      <RunsCard
        onOpen={onOpen}
        activeDetail={activeDetail}
        execPlan={workflow.plans.execution}
        runs={workflow.runs}
      />
      <ValidationCard onOpen={onOpen} activeDetail={activeDetail} />
      <ReleaseCard onOpen={onOpen} activeDetail={activeDetail} />
    </div>
  );
}
