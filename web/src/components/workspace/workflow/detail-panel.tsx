"use client";

import type { ReactNode } from "react";

import { IconArrow, IconX } from "@/components/icons";
import type { ExecutionRun, IntentDoc, PlanStep, StepStatus } from "@/lib/mock";
import { cn } from "@/lib/utils";

import { Markdown } from "./markdown";
import { statusMeta } from "./status-meta";
import type { DetailState } from "./types";

type Props = {
  detail: DetailState | null;
  onClose: () => void;
};

export function DetailPanel({ detail, onClose }: Props) {
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
        {detail && <DetailContent detail={detail} onClose={onClose} />}
      </div>
    </>
  );
}

function DetailContent({ detail, onClose }: { detail: DetailState; onClose: () => void }) {
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
        {detail.kind === "run" && <RunDetail run={detail.data} />}
        {detail.kind === "validation" && <ValidationDetail />}
        {detail.kind === "release" && <ReleaseDetail />}
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
  const md = [
    `# ${intent.title}`,
    "",
    intent.summary,
    "",
    "## Constraints",
    "",
    ...intent.constraints.map((c) => `- ${c}`),
    "",
    "## Context",
    "",
    `The caller surface is \`useMutation<T>\` in \`src/hooks/useMutation.ts\`, which today awaits \`fetcher(input)\` before touching the cache. Subscribers don't see the write until the round-trip finishes, so the UI feels sluggish on slow links.`,
    "",
    "## Approach",
    "",
    `1. Snapshot the cache entry under a per-key revision counter before the mutation fires.`,
    `2. Apply the optimistic patch synchronously so subscribers rerender immediately.`,
    `3. On success, reconcile the server response against the snapshot's revision — if a concurrent write landed first, keep the newer value.`,
    `4. On error, roll back to the snapshot and rethrow to \`onError\`.`,
    "",
    "## Out of scope",
    "",
    `- Changes to \`MutationOptions<T>\`'s public shape beyond adding an optional \`optimistic\` field.`,
    `- Server-driven cache invalidation — that stays in \`queryClient.invalidate\`.`,
  ].join("\n");

  return <Markdown source={md} />;
}

function PlanStepDetail({
  data,
}: {
  data: { step: PlanStep; planKind: "execution" | "test"; planId: string };
}) {
  const { step, planKind, planId } = data;
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
        <KV k="Kind" v={planKind === "test" ? "test" : "execution"} />
        <KV k="Status" v={step.status} />
      </Section>

      <Section label="Purpose">
        <div className="text-[12.5px] leading-[1.55] text-ink-2">
          {planKind === "test"
            ? "Verification step — asserts behavior after the execution DAG settles. Failures route back into a new plan revision."
            : "Execution step — mutates cache/code inside the sandbox. Output is captured as an append-only PatchSet bound to the parent plan."}
        </div>
      </Section>

      {step.status !== "queued" && (
        <Section label="Tool calls">
          <ToolCall name="read" arg="src/lib/query.ts" result="214 lines" />
          <ToolCall name="edit" arg="src/lib/query.ts" result="patchset ps-07" />
          {step.status === "running" && (
            <ToolCall
              name="test"
              arg="useMutation.test.ts"
              result="running…"
              running
            />
          )}
        </Section>
      )}

      <Section label="Sibling steps">
        <div className="text-[12px] text-ink-3">
          Linked into the plan DAG. Downstream gates won&apos;t open until this node reports DONE.
        </div>
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
        <KV k="Patch" v={run.patch} />
        <KV k="Finished" v={run.ago} />
        <KV k="Sandbox" v="libra-sbx-04 · rw" />
      </Section>

      <Section label="Output" mono>
        <pre className="mono m-0 whitespace-pre-wrap break-words rounded-md border border-rule bg-paper-2 p-3 text-[11px] leading-[1.55] text-ink">{`$ cargo test --lib optimistic
   Compiling libra-cache v0.3.1
    Finished test [unoptimized + debuginfo]
     Running tests/useMutation.test.ts
  ✓ snapshot captures prior cache state
  ✓ optimistic patch visible synchronously
  ${run.result === "running" ? "… revision-guarded rollback" : "✓ revision-guarded rollback"}
  ${run.result === "pass" ? "ok. 3 passed; 0 failed" : ""}`}</pre>
      </Section>

      <Section label="Patch">
        <div className="overflow-hidden rounded-md border border-rule">
          <div className="mono border-b border-rule bg-paper-2 px-3 py-1.5 text-[11px]">
            src/lib/query.ts ·{" "}
            <span className="mono text-ink-3">{run.patch}</span>
          </div>
          <pre className="mono m-0 whitespace-pre-wrap break-words bg-paper-2 p-3 text-[11px] leading-[1.55] text-ink">{`@@ useMutation ()
- const result = await fetcher(input);
- cache.set(key, result);
+ const snap = cache.snapshot(key);
+ cache.patch(key, optimistic);
+ try {
+   const result = await fetcher(input);
+   cache.reconcile(key, snap.rev, result);
+ } catch (err) {
+   cache.rollback(key, snap);
+   throw err;
+ }`}</pre>
        </div>
      </Section>
    </>
  );
}

function ValidationDetail() {
  return (
    <>
      <div className="mb-2.5 text-[15px] font-semibold">Validation gate</div>
      <div className="mb-[18px] text-[12.5px] leading-[1.6] text-ink-2">
        Phase 3 runs after the execution DAG settles. It audits the resulting PatchSet against policy and collects the evidence needed for release.
      </div>

      <Section label="Checks">
        <CheckRow name="SAST · static analysis" status="queued" />
        <CheckRow name="SCA · dependency advisories" status="queued" />
        <CheckRow name="Type-check" status="queued" />
        <CheckRow name="Test plan · full run" status="queued" />
        <CheckRow name="Compatibility · API surface" status="queued" />
      </Section>

      <Section label="Output">
        <div className="text-[12px] leading-[1.6] text-ink-3">
          Each check appends an Evidence record (kind ={" "}
          <span className="mono">audit</span>) to the thread&apos;s append-only log. The aggregate verdict determines whether Release auto-merges or escalates to human review.
        </div>
      </Section>
    </>
  );
}

function ReleaseDetail() {
  return (
    <>
      <div className="mb-2.5 text-[15px] font-semibold">Release decision</div>
      <div className="mb-[18px] text-[12.5px] leading-[1.6] text-ink-2">
        Phase 4 is the final decision. Libra classifies the PatchSet by risk, then either auto-merges or requests human review — producing a signed IntentEvent either way.
      </div>

      <Section label="Risk classification">
        <KV k="Policy" v="web3infra/default" />
        <KV k="Surface" v="internal hook · 2 callers" />
        <KV k="Blast radius" v="low" />
        <KV k="Reversibility" v="clean revert" />
      </Section>

      <Section label="Path">
        <div className="mb-2 flex items-center gap-1.5 text-[12.5px]">
          <span
            className="mono rounded-sm px-2 py-px text-[10.5px]"
            style={{ background: "var(--good-soft)", color: "var(--good)" }}
          >
            LOW
          </span>
          <IconArrow size={11} className="text-ink-3" />
          <span>
            Auto-merge to <span className="mono">main</span>
          </span>
        </div>
        <div className="flex items-center gap-1.5 text-[12.5px] text-ink-3">
          <span
            className="mono rounded-sm px-2 py-px text-[10.5px]"
            style={{ background: "var(--warn-soft)", color: "var(--warn)" }}
          >
            HIGH
          </span>
          <IconArrow size={11} />
          <span>Open review for erin@web3infra</span>
        </div>
      </Section>

      <Section label="Output">
        <div className="text-[12px] leading-[1.6] text-ink-3">
          Decision is sealed as an <span className="mono">IntentEvent</span> on the thread and mirrored to the git provider. No phase can run past Release without a decision record.
        </div>
      </Section>
    </>
  );
}

function ToolCall({
  name,
  arg,
  result,
  running,
}: {
  name: string;
  arg: string;
  result: string;
  running?: boolean;
}) {
  return (
    <div className="flex items-center gap-2 border-b border-rule py-1.5">
      <span className="mono text-[10.5px] font-semibold text-accent">{name}</span>
      <span className="mono flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[11px] text-ink">
        {arg}
      </span>
      <span
        className={cn(
          "mono inline-flex items-center gap-1 text-[10.5px]",
          running ? "text-accent" : "text-ink-3",
        )}
      >
        {running && <span className="libra-spin" />} {result}
      </span>
    </div>
  );
}

function CheckRow({ name, status }: { name: string; status: StepStatus }) {
  const m = statusMeta(status);
  return (
    <div className="flex items-center gap-2 border-b border-rule py-2">
      <div
        className="grid h-4 w-4 place-items-center rounded-full border"
        style={{ background: m.bg, color: m.color, borderColor: m.color }}
      >
        {m.icon}
      </div>
      <span className="flex-1 text-[12.5px]">{name}</span>
      <span className="mono text-[10px]" style={{ color: m.color }}>
        {m.label}
      </span>
    </div>
  );
}
