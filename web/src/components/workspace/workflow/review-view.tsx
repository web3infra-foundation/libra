"use client";

import { useMemo, useState } from "react";

import { IconChev } from "@/components/icons";
import { useCodeUiStore } from "@/lib/code-ui/store";
import { cn } from "@/lib/utils";

import { parseUnifiedDiff, type DiffFile, type DiffLine } from "./diff-parser";

const MAX_RENDERED_DIFF_LINES = 300;
const MAX_RENDERED_RAW_DIFF_CHARS = 20_000;

export function ReviewView() {
  const { snapshot } = useCodeUiStore();
  const files = useMemo<DiffFile[]>(() => {
    if (!snapshot) return [];
    return snapshot.patchsets.flatMap((patchset) =>
      patchset.changes.map((change) =>
        parseUnifiedDiff(change.path, change.diff ?? null, change.changeType),
      ),
    );
  }, [snapshot]);

  if (files.length === 0) {
    return (
      <div className="px-[18px] pb-6 pt-4 text-[12.5px] italic text-ink-3">
        No PatchSet diffs to review yet.
      </div>
    );
  }

  const totals = files.reduce(
    (acc, f) => ({ files: acc.files + 1, add: acc.add + f.add, del: acc.del + f.del }),
    { files: 0, add: 0, del: 0 },
  );

  return (
    <div className="px-[18px] pb-6 pt-4">
      <div className="mb-1 flex items-center gap-2.5 px-0.5 pb-2.5">
        <span className="mono text-[11px] text-ink-3">{totals.files} files</span>
        <span className="mono text-[11px] text-good">+{totals.add}</span>
        <span className="mono text-[11px] text-bad">−{totals.del}</span>
      </div>
      {files.map((f) => (
        <FileDiff key={`${f.path}-${f.changeType}`} file={f} />
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
        <span className="mono text-[10.5px] text-ink-3">{file.changeType}</span>
        <span className="mono text-[10.5px] text-good">+{file.add}</span>
        <span className="mono text-[10.5px] text-bad">−{file.del}</span>
      </button>
      {open && file.parseError && (
        <div className="mono border-b border-rule bg-paper-2 px-2.5 py-1 text-[10.5px] text-bad">
          unable to parse diff: {file.parseError}
        </div>
      )}
      {open &&
        file.hunks.map((h, i) => {
          const visibleLines = h.lines.slice(0, MAX_RENDERED_DIFF_LINES);
          const hiddenLines = h.lines.length - visibleLines.length;
          return (
            <div key={i}>
              <div className="mono border-b border-rule bg-paper-2 px-2.5 py-1 text-[10.5px] text-ink-3">
                {h.header}
              </div>
              <div>
                {visibleLines.map((ln, j) => (
                  <DiffLineRow key={j} line={ln} />
                ))}
                {hiddenLines > 0 && (
                  <CollapsedDiffNotice hiddenLines={hiddenLines} />
                )}
              </div>
            </div>
          );
        })}
      {open && file.hunks.length === 0 && file.rawDiff && (
        <>
          <pre className="mono m-0 whitespace-pre-wrap break-words bg-paper-2 px-2.5 py-2 text-[11px] leading-[1.5] text-ink">
            {file.rawDiff.slice(0, MAX_RENDERED_RAW_DIFF_CHARS)}
          </pre>
          {file.rawDiff.length > MAX_RENDERED_RAW_DIFF_CHARS && (
            <CollapsedDiffNotice
              hiddenChars={file.rawDiff.length - MAX_RENDERED_RAW_DIFF_CHARS}
            />
          )}
        </>
      )}
      {open && file.hunks.length === 0 && !file.rawDiff && (
        <div className="px-2.5 py-2 text-[11px] italic text-ink-3">
          No inline diff for this change.
        </div>
      )}
    </div>
  );
}

function CollapsedDiffNotice({
  hiddenLines,
  hiddenChars,
}: {
  hiddenLines?: number;
  hiddenChars?: number;
}) {
  const label =
    hiddenLines !== undefined
      ? `${hiddenLines} more diff lines hidden`
      : `${hiddenChars ?? 0} more raw diff characters hidden`;
  return (
    <div className="mono border-t border-rule bg-paper-2 px-2.5 py-1.5 text-[10.5px] text-ink-3">
      Diff collapsed: {label}.
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
