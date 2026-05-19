/**
 * Account / settings popover anchored above the sidebar avatar button.
 *
 * Layout-only component — the items don't yet wire to real actions; clicks
 * inside the popover are stopped from propagating so the parent's
 * outside-click listener (in `sidebar.tsx`) doesn't immediately close it.
 */
"use client";

import { cn } from "@/lib/utils";

/** Props for a single row inside the settings menu. */
type ItemProps = {
  /** Primary text. */
  label: string;
  /** Right-aligned secondary text (e.g. account email or rate-limit %). */
  meta?: string;
  /** Right-aligned keyboard shortcut chip. */
  shortcut?: string;
  /** Highlights the row as the currently selected option. */
  active?: boolean;
  /** Tints the row as a destructive action. */
  danger?: boolean;
  /** Renders `meta` in monospace (used for percentages and short codes). */
  mono?: boolean;
};

/** Single row inside the settings menu. */
function MenuItem({ label, meta, shortcut, active, danger, mono }: ItemProps) {
  return (
    <button
      type="button"
      className={cn(
        "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left",
        danger ? "text-bad" : "text-ink",
        active && "bg-paper-2",
      )}
    >
      <span className="flex-1 text-[12.5px]">{label}</span>
      {meta && (
        <span
          className={cn(
            "text-[10.5px] text-ink-3",
            mono && "mono",
          )}
        >
          {meta}
        </span>
      )}
      {shortcut && (
        <span className="mono rounded-sm border border-rule bg-paper-2 px-1.5 py-[1px] text-[10px] text-ink-3">
          {shortcut}
        </span>
      )}
    </button>
  );
}

/**
 * Floating panel showing account info, account switcher, and quick links.
 *
 * Positioned absolutely above the avatar button in the sidebar footer. The
 * outer click handler stops bubbling so the sidebar's outside-click guard
 * does not dismiss the menu when the user clicks inside it.
 */
export function SettingsMenu() {
  return (
    <div
      onClick={(e) => e.stopPropagation()}
      className="absolute bottom-[calc(100%+8px)] left-0 z-40 w-60 rounded-lg border border-rule-2 bg-paper p-1.5 shadow-[0_12px_32px_-12px_rgba(0,0,0,0.18),0_2px_6px_rgba(0,0,0,0.05)]"
    >
      <div className="mb-1 flex items-center gap-2.5 border-b border-rule px-2 pb-2.5 pt-1.5">
        <div className="grid h-8 w-8 place-items-center rounded-full bg-ink text-[11px] font-semibold text-paper">
          EC
        </div>
        <div className="min-w-0 flex-1">
          <div className="text-[12.5px] font-semibold">Erin Chen</div>
          <div className="text-[10.5px] text-ink-3">erin@web3infra.io</div>
        </div>
      </div>
      <div className="flex flex-col gap-px">
        <MenuItem label="Personal account" meta="erin@web3infra.io" active />
        <MenuItem label="web3infra / libra" meta="team" />
      </div>
      <div className="-mx-1.5 my-1 h-px bg-rule" />
      <MenuItem label="Settings" shortcut="⌘," />
      <MenuItem label="Integrations" />
      <MenuItem label="Rate limits remaining" meta="84%" mono />
      <div className="-mx-1.5 my-1 h-px bg-rule" />
      <MenuItem label="Keyboard shortcuts" shortcut="⌘/" />
      <MenuItem label="Documentation" />
    </div>
  );
}
