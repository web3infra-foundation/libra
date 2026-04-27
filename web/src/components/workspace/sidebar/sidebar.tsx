/**
 * Left-hand sidebar: brand mark, "New thread" CTA, search box, scrollable
 * thread list, and the account/settings popover trigger at the bottom.
 *
 * Holds local UI state for: search query, settings popover visibility, and
 * the currently selected thread id (initialised to the thread flagged
 * `active` in the mock data, falling back to the first row).
 *
 * Width is provided by the parent so the surrounding {@link Splitter} can
 * resize the sidebar without re-mounting the tree.
 */
"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import { BrandMark } from "@/components/workspace/brand-mark";
import { IconPlus, IconSearch, IconSettings } from "@/components/icons";
import { PHASES, THREADS, type Thread } from "@/lib/mock";
import { cn } from "@/lib/utils";

import { SettingsMenu } from "./settings-menu";
import { ThreadItem } from "./thread-item";

/** Sidebar props. */
type Props = {
  /** Pixel width controlled by the parent {@link Workspace}. */
  width: number;
};

/**
 * Sidebar component.
 *
 * Effects:
 * - When the settings popover is open, listens for `mousedown` outside the
 *   anchor element and `Escape` to close it. Listeners are removed when the
 *   popover closes or the component unmounts.
 *
 * Boundary: an empty filter result renders an italic "No threads match." line
 * rather than collapsing the list area, so the empty state is visible.
 */
export function Sidebar({ width }: Props) {
  const [query, setQuery] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const [activeId, setActiveId] = useState(
    () => THREADS.find((t) => t.active)?.id ?? THREADS[0]?.id,
  );
  // Anchor element for outside-click detection on the popover.
  const avatarRef = useRef<HTMLDivElement | null>(null);

  // Case-insensitive substring filter against thread titles. Empty query
  // returns the full list unchanged.
  const filtered: Thread[] = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return THREADS;
    return THREADS.filter((t) => t.title.toLowerCase().includes(q));
  }, [query]);

  // Outside-click + Escape dismissal for the settings popover. Only mounts
  // its global listeners while the popover is open to avoid background work
  // when it is dismissed.
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
        {filtered.map((t) => (
          <ThreadItem
            key={t.id}
            thread={t}
            phaseLabel={PHASES[t.phase]?.label ?? "Phase"}
            active={t.id === activeId}
            onSelect={() => setActiveId(t.id)}
          />
        ))}
        {filtered.length === 0 && (
          <div className="px-1 py-2 text-[12px] italic text-ink-3">
            No threads match.
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
              web3infra / libra
            </div>
            <div className="text-[11px] text-ink-3">main · clean</div>
          </div>
          <IconSettings size={14} />
          {menuOpen && <SettingsMenu />}
        </div>
      </div>
    </aside>
  );
}
