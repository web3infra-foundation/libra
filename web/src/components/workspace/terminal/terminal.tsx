"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import { IconSpark, IconTerm, IconTool, IconX } from "@/components/icons";
import { getDiagnostics } from "@/lib/code-ui/client";
import { useCodeUiStore } from "@/lib/code-ui/store";
import type { CodeUiDiagnostics } from "@/lib/code-ui/types";
import { deriveTerminalRows, type TerminalRow } from "@/lib/code-ui/view-model";
import { cn } from "@/lib/utils";

type Tab = "sandbox" | "tools" | "agent";

type Props = {
  height: number;
  onClose: () => void;
};

export function Terminal({ height, onClose }: Props) {
  const { snapshot, connection } = useCodeUiStore();
  const [tab, setTab] = useState<Tab>("sandbox");
  const [diagnostics, setDiagnostics] = useState<CodeUiDiagnostics | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let cancelled = false;
    getDiagnostics()
      .then((value) => {
        if (!cancelled) setDiagnostics(value);
      })
      .catch(() => {
        // Diagnostics are observe-only; terminal output should keep rendering
        // even when the runtime is unavailable or the diagnostics endpoint
        // rejects the request.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const rows = useMemo(
    () => [
      ...deriveTerminalRows(snapshot),
      ...deriveDiagnosticsRows(diagnostics),
    ],
    [snapshot, diagnostics],
  );

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [rows]);

  const visible = rows.filter((row) => filterByTab(row, tab));
  const sandboxBadge = snapshot
    ? `${snapshot.provider.provider}${snapshot.provider.model ? ` · ${snapshot.provider.model}` : ""}`
    : "no session";
  const sandboxState =
    connection.kind === "ready" ? "live" : connection.kind === "loading" ? "loading…" : connection.kind;

  return (
    <div
      className="flex shrink-0 flex-col overflow-hidden border-t border-rule-2 bg-paper-2"
      style={{ height, minHeight: 120 }}
    >
      <header className="flex h-[34px] shrink-0 items-center justify-between border-b border-rule bg-paper px-4 py-0">
        <div className="flex gap-px">
          <TermTab active={tab === "sandbox"} onClick={() => setTab("sandbox")}>
            <IconTerm size={12} /> Sandbox
          </TermTab>
          <TermTab active={tab === "tools"} onClick={() => setTab("tools")}>
            <IconTool size={12} /> Tools
          </TermTab>
          <TermTab active={tab === "agent"} onClick={() => setTab("agent")}>
            <IconSpark size={12} /> Agent
          </TermTab>
        </div>
        <div className="flex items-center gap-1.5 text-[11px] text-ink-3">
          <span
            className="h-[7px] w-[7px] rounded-full bg-good"
            style={{ boxShadow: "0 0 0 2px color-mix(in oklch, var(--good) 22%, transparent)" }}
          />
          <span className="mono text-[10.5px]">{sandboxBadge}</span>
          <span className="text-rule-2">·</span>
          <span className="mono text-[10.5px]">{sandboxState}</span>
          <button
            type="button"
            onClick={onClose}
            className="ml-1 grid h-[22px] w-[22px] place-items-center rounded-sm text-ink-3"
            title="Hide terminal"
          >
            <IconX size={12} />
          </button>
        </div>
      </header>

      <div ref={scrollRef} className="flex-1 overflow-y-auto bg-paper-2 px-4 py-2">
        {visible.length === 0 && (
          <div className="text-[11.5px] italic text-ink-3">
            No tool output yet — the agent will surface tool calls here.
          </div>
        )}
        {visible.map((row, i) => (
          <TermLineRow key={i} row={row} />
        ))}
      </div>

      <div className="flex shrink-0 items-center gap-2 border-t border-rule bg-paper px-4 py-2 text-[11px] text-ink-3">
        <span className="mono shrink-0 text-[11px] font-medium text-ink-3">agent@sandbox</span>
        <span className="flex-1 italic">
          Browser-side shell input is disabled in v1 — drive commands through the agent message panel.
        </span>
      </div>
    </div>
  );
}

function TermTab({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "-mb-px inline-flex items-center gap-1.5 border-b-[1.5px] px-2.5 py-1 text-[11.5px] font-medium",
        active ? "border-ink text-ink" : "border-transparent text-ink-3",
      )}
    >
      {children}
    </button>
  );
}

function TermLineRow({ row }: { row: TerminalRow }) {
  const tone = rowTone(row.kind);
  const [expanded, setExpanded] = useState(false);
  const displayedText = expanded && row.fullText ? row.fullText : row.text;
  return (
    <div className="flex items-baseline gap-2 py-[1.5px]">
      <span
        className="mono w-3.5 shrink-0 text-[10.5px]"
        style={{ color: tone.mark }}
      >
        {rowMark(row.kind)}
      </span>
      <span className="flex-1">
        <span
          className="mono block whitespace-pre-wrap break-words text-[11.5px] leading-[1.55]"
          style={{ color: tone.text }}
        >
          {displayedText || " "}
        </span>
        {row.fullText && (
          <button
            type="button"
            onClick={() => setExpanded((value) => !value)}
            className="mt-1 rounded-sm border border-rule bg-paper px-1.5 py-0.5 text-[10.5px] font-medium text-ink-3 hover:text-ink"
          >
            {expanded ? "Show less" : `Show full output (${row.hiddenChars?.toLocaleString()} chars hidden)`}
          </button>
        )}
      </span>
    </div>
  );
}

function rowMark(kind: TerminalRow["kind"]) {
  switch (kind) {
    case "pass":
      return "✓";
    case "fail":
      return "✗";
    case "run":
      return "•";
    case "warn":
      return "!";
    case "info":
      return "ℹ";
    case "meta":
      return "·";
    default:
      return " ";
  }
}

function rowTone(kind: TerminalRow["kind"]) {
  switch (kind) {
    case "pass":
      return { mark: "var(--good)", text: "var(--ink-2)" };
    case "fail":
      return { mark: "var(--bad)", text: "var(--bad)" };
    case "run":
      return { mark: "var(--accent)", text: "var(--ink-2)" };
    case "warn":
      return { mark: "var(--warn)", text: "var(--ink-2)" };
    case "info":
      return { mark: "var(--accent)", text: "var(--ink-2)" };
    case "meta":
      return { mark: "var(--ink-3)", text: "var(--ink-3)" };
    default:
      return { mark: "var(--ink-3)", text: "var(--ink-2)" };
  }
}

function deriveDiagnosticsRows(
  diagnostics: CodeUiDiagnostics | null,
): TerminalRow[] {
  if (!diagnostics) return [];

  const parts = [
    `diagnostics: pid ${diagnostics.pid}`,
    `status ${diagnostics.status}`,
  ];
  if (diagnostics.threadId) parts.push(`thread ${diagnostics.threadId}`);
  if (diagnostics.ports?.web !== undefined) {
    parts.push(`web ${diagnostics.ports.web}`);
  }
  if (diagnostics.ports?.mcp !== undefined) {
    parts.push(`mcp ${diagnostics.ports.mcp}`);
  }
  if (diagnostics.logFile) parts.push(`log ${diagnostics.logFile}`);

  const rows: TerminalRow[] = [{ kind: "info", text: parts.join(" · ") }];
  if (diagnostics.activeInteractionId) {
    rows.push({
      kind: "info",
      text: `active interaction ${diagnostics.activeInteractionId}`,
    });
  }
  if (diagnostics.lastError) {
    rows.push({ kind: "warn", text: `last error: ${diagnostics.lastError}` });
  }
  return rows;
}

function filterByTab(row: TerminalRow, tab: Tab) {
  if (tab === "sandbox") return true;
  if (tab === "tools") return row.kind === "run" || row.kind === "pass" || row.kind === "fail" || row.kind === "meta";
  if (tab === "agent") return row.kind === "info" || row.kind === "warn" || row.kind === "meta";
  return true;
}
