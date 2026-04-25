"use client";

import type { ReactNode } from "react";

import { IconCheck } from "@/components/icons";
import { SUMMARY } from "@/lib/mock";
import { cn } from "@/lib/utils";

export function SummaryView() {
  return (
    <div className="px-[18px] pb-6 pt-4">
      <Block label="Progress">
        <ul className="m-0 list-none p-0">
          {SUMMARY.progress.map((p, i) => (
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

      <Block label="Branch state">
        <Row label="Branch">
          <span className="mono">{SUMMARY.branch.name}</span>
        </Row>
        <Row label="Base">
          <span className="mono">{SUMMARY.branch.base}</span>
        </Row>
        <Row label="PR">
          <span>{SUMMARY.branch.pr}</span>
        </Row>
        <Row label="Changes">
          <span>{SUMMARY.branch.changes}</span>
        </Row>
      </Block>

      <Block label="Artifacts">
        {SUMMARY.artifacts.map((a, i) => (
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
          {SUMMARY.todo.map((t, i) => (
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

function Block({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="mb-5">
      <div className="mb-2 text-[10px] font-medium uppercase tracking-[0.08em] text-ink-3">
        {label}
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
