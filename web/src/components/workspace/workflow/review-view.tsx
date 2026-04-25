"use client";

import { useState } from "react";

import { IconChev } from "@/components/icons";
import { REVIEW, type DiffFile, type DiffLine } from "@/lib/mock";
import { cn } from "@/lib/utils";

export function ReviewView() {
  return (
    <div className="px-[18px] pb-6 pt-4">
      <div className="mb-1 flex items-center gap-2.5 px-0.5 pb-2.5">
        <span className="mono text-[11px] text-ink-3">
          {REVIEW.stats.files} files
        </span>
        <span className="mono text-[11px] text-good">+{REVIEW.stats.add}</span>
        <span className="mono text-[11px] text-bad">−{REVIEW.stats.del}</span>
      </div>
      {REVIEW.files.map((f) => (
        <FileDiff key={f.path} file={f} />
      ))}
    </div>
  );
}

function FileDiff({ file }: { file: DiffFile }) {
  const [open, setOpen] = useState(true);
  return (
    <div className="mb-2.5 overflow-hidden rounded-md border border-rule bg-paper">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-2 border-b border-rule bg-paper-2 px-2.5 py-2"
      >
        <div
          className={cn(
            "text-ink-3 transition-transform duration-150",
            open ? "rotate-90" : "rotate-0",
          )}
        >
          <IconChev size={12} />
        </div>
        <span className="mono flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-left text-[11.5px]">
          {file.path}
        </span>
        <span className="mono text-[10.5px] text-good">+{file.add}</span>
        <span className="mono text-[10.5px] text-bad">−{file.del}</span>
      </button>
      {open &&
        file.hunks.map((h, i) => (
          <div key={i}>
            <div className="mono border-b border-rule bg-paper-2 px-2.5 py-1 text-[10.5px] text-ink-3">
              {h.header}
            </div>
            <div>
              {h.lines.map((ln, j) => (
                <DiffLineRow key={j} line={ln} />
              ))}
            </div>
          </div>
        ))}
    </div>
  );
}

function DiffLineRow({ line }: { line: DiffLine }) {
  const bg =
    line.kind === "add"
      ? "color-mix(in oklch, var(--good) 10%, var(--paper))"
      : line.kind === "del"
        ? "color-mix(in oklch, var(--bad) 10%, var(--paper))"
        : "transparent";
  const marker = line.kind === "add" ? "+" : line.kind === "del" ? "−" : " ";
  const markerColor =
    line.kind === "add"
      ? "var(--good)"
      : line.kind === "del"
        ? "var(--bad)"
        : "var(--ink-3)";
  return (
    <div
      className="mono flex text-[11px] leading-[1.5]"
      style={{ background: bg }}
    >
      <span className="w-9 shrink-0 border-r border-rule px-1.5 text-right text-[10px] text-ink-3">
        {line.n1 ?? ""}
      </span>
      <span className="w-9 shrink-0 border-r border-rule px-1.5 text-right text-[10px] text-ink-3">
        {line.n2 ?? ""}
      </span>
      <span
        className="w-3.5 text-center"
        style={{ color: markerColor }}
      >
        {marker}
      </span>
      <span className="flex-1 whitespace-pre pr-2.5 text-ink">{line.text}</span>
    </div>
  );
}
