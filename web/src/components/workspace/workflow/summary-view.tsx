"use client";

import type { ReactNode } from "react";

import { IconCheck } from "@/components/icons";
import { useCodeUiStore } from "@/lib/code-ui/store";
import { cn } from "@/lib/utils";

type ProgressItem = { done: boolean; text: string };
type ArtifactItem = { kind: string; id: string; meta: string };

export function SummaryView() {
  const { snapshot, status, refreshStatus } = useCodeUiStore();

  const progress: ProgressItem[] = snapshot
    ? buildProgress(snapshot)
    : [];
  const artifacts: ArtifactItem[] = snapshot
    ? buildArtifacts(snapshot)
    : [];
  const todos: ProgressItem[] = snapshot
    ? buildTodos(snapshot)
    : [];

  const branch = status?.head.type === "branch"
    ? status.head.name
    : status
      ? `detached @ ${status.head.oid.slice(0, 7)}`
      : "—";
  const upstream = status?.upstream?.remote_ref ?? "no upstream";
  const dirtyChanges =
    status === null
      ? "—"
      : (() => {
          const total =
            status.staged.new.length +
            status.staged.modified.length +
            status.staged.deleted.length +
            status.unstaged.modified.length +
            status.unstaged.deleted.length +
            status.untracked.length;
          if (status.is_clean) return "clean";
          return `${total} files`;
        })();
  const aheadBehind = status?.upstream
    ? `↑${status.upstream.ahead ?? "—"} ↓${status.upstream.behind ?? "—"}`
    : "—";

  return (
    <div className="px-[18px] pb-6 pt-4">
      <Block label="Progress">
        <ul className="m-0 list-none p-0">
          {progress.length === 0 && <EmptyHint />}
          {progress.map((p, i) => (
            <li
              key={i}
              className="flex items-start gap-2 py-[5px] text-[12.5px] leading-[1.5]"
            >
              <Tick on={p.done} />
              <span className={cn(p.done ? "text-ink" : "text-ink-2")}>
                {p.text}
              </span>
            </li>
          ))}
        </ul>
      </Block>

      <Block
        label="Branch state"
        action={
          <button
            type="button"
            onClick={() => void refreshStatus()}
            className="text-[10.5px] text-ink-3 underline-offset-2 hover:underline"
            title="Refresh repo status"
          >
            refresh
          </button>
        }
      >
        <Row label="Branch">
          <span className="mono">{branch}</span>
        </Row>
        <Row label="Upstream">
          <span className="mono">{upstream}</span>
        </Row>
        <Row label="Ahead / behind">
          <span className="mono">{aheadBehind}</span>
        </Row>
        <Row label="Changes">
          <span>{dirtyChanges}</span>
        </Row>
      </Block>

      <Block label="Artifacts">
        {artifacts.length === 0 && <EmptyHint />}
        {artifacts.map((a, i) => (
          <div
            key={i}
            className="mb-1 flex items-center gap-2 rounded-md border border-rule bg-paper-2 px-2 py-1.5"
          >
            <span className="mono rounded-sm border border-rule-2 bg-paper px-1.5 py-px text-[9.5px] tracking-[0.04em] text-ink-2">
              {a.kind}
            </span>
            <span className="mono text-[11.5px]">{a.id}</span>
            <span className="ml-auto text-[11.5px] text-ink-3">{a.meta}</span>
          </div>
        ))}
      </Block>

      <Block label="To-dos">
        <ul className="m-0 list-none p-0">
          {todos.length === 0 && <EmptyHint />}
          {todos.map((t, i) => (
            <li
              key={i}
              className="flex items-start gap-2 py-[5px] text-[12.5px] leading-[1.5]"
            >
              <Tick on={t.done} />
              <span
                className={cn(
                  t.done ? "text-ink-3 line-through" : "text-ink",
                )}
              >
                {t.text}
              </span>
            </li>
          ))}
        </ul>
      </Block>
    </div>
  );
}

function buildProgress(
  snapshot: NonNullable<ReturnType<typeof useCodeUiStore>["snapshot"]>,
): ProgressItem[] {
  return snapshot.plans.flatMap((plan) =>
    plan.steps.map((step) => ({
      done: step.status === "done" || step.status === "completed",
      text: step.step,
    })),
  );
}

function buildArtifacts(
  snapshot: NonNullable<ReturnType<typeof useCodeUiStore>["snapshot"]>,
): ArtifactItem[] {
  return snapshot.patchsets.map((patchset) => ({
    kind: "PatchSet",
    id: patchset.id,
    meta: `${patchset.changes.length} files · ${patchset.status}`,
  }));
}

function buildTodos(
  snapshot: NonNullable<ReturnType<typeof useCodeUiStore>["snapshot"]>,
): ProgressItem[] {
  return snapshot.tasks.map((task) => ({
    done: task.status === "done" || task.status === "completed",
    text: task.title ?? task.id,
  }));
}

function Block({
  label,
  children,
  action,
}: {
  label: string;
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="mb-5">
      <div className="mb-2 flex items-center justify-between gap-2">
        <span className="text-[10px] font-medium uppercase tracking-[0.08em] text-ink-3">
          {label}
        </span>
        {action}
      </div>
      {children}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex justify-between border-b border-rule py-[5px] text-[12px] text-ink-2">
      <span>{label}</span>
      {children}
    </div>
  );
}

function Tick({ on }: { on: boolean }) {
  return (
    <span
      className={cn(
        "mt-0.5 grid h-3.5 w-3.5 shrink-0 place-items-center rounded-sm border text-white",
        on ? "border-accent bg-accent" : "border-rule-2 bg-paper",
      )}
    >
      {on && <IconCheck size={9} sw={3} />}
    </span>
  );
}

function EmptyHint() {
  return (
    <div className="text-[12px] italic text-ink-3">No data for this section yet.</div>
  );
}
