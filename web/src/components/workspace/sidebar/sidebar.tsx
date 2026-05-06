/**
 * Left-hand sidebar: brand mark, "New thread" CTA, search box, scrollable
 * thread list, and the account/settings popover trigger at the bottom.
 *
 * Phase 1 wires the active thread to the live `CodeUiSessionSnapshot`.
 * Historical thread list lands in Phase 4 once `/api/code/threads` exists.
 */
"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import { BrandMark } from "@/components/workspace/brand-mark";
import { IconPlus, IconSearch, IconSettings } from "@/components/icons";
import { useCodeUiStore } from "@/lib/code-ui/store";
import { PHASES } from "@/lib/code-ui/phases";
import { cn } from "@/lib/utils";

import { SettingsMenu } from "./settings-menu";
import { ThreadItem, type ThreadRow } from "./thread-item";

/** Sidebar props. */
type Props = {
  /** Pixel width controlled by the parent {@link Workspace}. */
  width: number;
};

export function Sidebar({ width }: Props) {
  const { snapshot, repo, status, connection } = useCodeUiStore();

  const [query, setQuery] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const avatarRef = useRef<HTMLDivElement | null>(null);

  // Phase 1: only the active session is known. Historical thread list comes
  // from `/api/code/threads` in Phase 4.
  const activeThread: ThreadRow | null = useMemo(() => {
    if (!snapshot?.threadId) return null;
    return {
      id: snapshot.threadId,
      title: deriveSessionTitle(snapshot, repo?.name ?? null),
      ago: deriveSessionAgo(snapshot.updatedAt),
      phase: statusToPhaseIndex(snapshot.status),
    };
  }, [snapshot, repo]);

  const visibleThreads = useMemo(() => {
    if (!activeThread) return [] as ThreadRow[];
    const q = query.trim().toLowerCase();
    if (q && !activeThread.title.toLowerCase().includes(q)) return [];
    return [activeThread];
  }, [activeThread, query]);

  useEffect(() => {
    if (!menuOpen) return;
    function onDown(e: MouseEvent) {
      if (
        avatarRef.current &&
        e.target instanceof Node &&
        !avatarRef.current.contains(e.target)
      ) {
        setMenuOpen(false);
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setMenuOpen(false);
    }
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [menuOpen]);

  const repoLine = deriveRepoLine(repo, status);

  return (
    <aside
      className="flex shrink-0 flex-col border-r border-rule bg-paper-2 px-3 py-3.5"
      style={{ width }}
    >
      <div className="flex items-center gap-2.5 px-1 pb-3.5 pt-0.5">
        <div className="grid h-7 w-7 place-items-center">
          <BrandMark size={26} />
        </div>
        <div>
          <div className="font-semibold tracking-tight">Libra</div>
          <div className="text-[11px] text-ink-3">agent workspace</div>
        </div>
      </div>

      <button
        type="button"
        title="Start a new libra code thread from your terminal"
        className="mb-2.5 flex w-full items-center gap-2 rounded-md border border-rule-2 bg-paper px-2.5 py-2 text-[12.5px] font-medium text-ink"
      >
        <IconPlus size={14} sw={2} /> New thread
        <span className="mono ml-auto rounded-sm border border-rule bg-paper-2 px-1.5 py-[2px] text-[10px] text-ink-3">
          ⌘N
        </span>
      </button>

      <div className="mb-3.5 flex items-center gap-1.5 rounded-md border border-rule bg-paper px-2.5 py-1.5 text-ink-3">
        <IconSearch size={14} />
        <input
          placeholder="Search threads"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="flex-1 border-none bg-transparent text-[12.5px] text-ink outline-none placeholder:text-ink-3"
        />
      </div>

      <div className="px-1 pb-2 text-[10px] font-medium uppercase tracking-[0.08em] text-ink-3">
        Threads
      </div>
      <div className="-mx-1 flex-1 overflow-y-auto px-1">
        {visibleThreads.map((thread) => (
          <ThreadItem
            key={thread.id}
            thread={thread}
            phaseLabel={
              typeof thread.phase === "number"
                ? PHASES[thread.phase]?.label
                : undefined
            }
            active={true}
            onSelect={() => undefined}
          />
        ))}
        {visibleThreads.length === 0 && (
          <div className="px-1 py-2 text-[12px] italic text-ink-3">
            {connection.kind === "loading"
              ? "Loading…"
              : connection.kind === "unavailable"
                ? "No active libra code session"
                : query.trim()
                  ? "No threads match."
                  : "No threads yet — start one in the libra TUI."}
          </div>
        )}
      </div>

      <div className="mt-2 border-t border-rule pt-2.5">
        <div ref={avatarRef} className="relative flex items-center gap-2.5 px-0.5 py-1 text-ink-2">
          <button
            type="button"
            onClick={() => setMenuOpen((o) => !o)}
            className={cn(
              "grid h-[26px] w-[26px] shrink-0 cursor-pointer place-items-center rounded-full bg-ink text-[10px] font-semibold text-paper",
              menuOpen && "outline-2 outline-offset-1 outline-accent-line",
            )}
            title="Account"
          >
            EC
          </button>
          <div className="min-w-0 flex-1">
            <div className="overflow-hidden text-ellipsis whitespace-nowrap text-[12px] font-medium">
              {repoLine.name}
            </div>
            <div className="text-[11px] text-ink-3">{repoLine.detail}</div>
          </div>
          <IconSettings size={14} />
          {menuOpen && <SettingsMenu />}
        </div>
      </div>
    </aside>
  );
}

function deriveSessionTitle(
  snapshot: ReturnType<typeof useCodeUiStore>["snapshot"],
  repoName: string | null,
): string {
  if (!snapshot) return "libra code";
  if (repoName) return repoName;
  return snapshot.threadId ?? "libra code";
}

function deriveSessionAgo(updatedAt: string | undefined): string {
  if (!updatedAt) return "";
  const updated = new Date(updatedAt).getTime();
  if (Number.isNaN(updated)) return "";
  const seconds = Math.max(0, Math.floor((Date.now() - updated) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

function statusToPhaseIndex(
  status: NonNullable<ReturnType<typeof useCodeUiStore>["snapshot"]>["status"],
): number | undefined {
  switch (status) {
    case "thinking":
      return 0;
    case "executing_tool":
      return 2;
    case "awaiting_interaction":
      return 1;
    case "completed":
      return 4;
    case "error":
      return 3;
    default:
      return undefined;
  }
}

function deriveRepoLine(
  repo: ReturnType<typeof useCodeUiStore>["repo"],
  status: ReturnType<typeof useCodeUiStore>["status"],
): { name: string; detail: string } {
  const name = repo?.name?.trim() || "libra";
  if (!status) {
    return { name, detail: "loading status…" };
  }
  const branch =
    status.head.type === "branch"
      ? status.head.name
      : `detached @ ${status.head.oid.slice(0, 7)}`;
  const stateBits: string[] = [branch];
  stateBits.push(status.is_clean ? "clean" : "dirty");
  if (status.upstream) {
    const ahead = status.upstream.ahead ?? 0;
    const behind = status.upstream.behind ?? 0;
    if (ahead > 0) stateBits.push(`↑${ahead}`);
    if (behind > 0) stateBits.push(`↓${behind}`);
  }
  return { name, detail: stateBits.join(" · ") };
}
