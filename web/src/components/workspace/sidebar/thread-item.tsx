"use client";

import { cn } from "@/lib/utils";
import type { Thread } from "@/lib/mock";

type Props = {
  thread: Thread;
  phaseLabel: string;
  active: boolean;
  onSelect: () => void;
};

export function ThreadItem({ thread, phaseLabel, active, onSelect }: Props) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        "mb-0.5 flex w-full items-start gap-2 rounded-md py-2 pl-1.5 pr-2 text-left transition-colors",
        active && "bg-paper",
      )}
    >
      <div className="relative my-0.5 w-0.5 self-stretch overflow-hidden rounded-sm">
        {active && <div className="absolute inset-0 rounded-sm bg-accent" />}
      </div>
      <div className="min-w-0 flex-1">
        <div
          className={cn(
            "overflow-hidden text-ellipsis whitespace-nowrap text-[12.5px] text-ink",
            active ? "font-medium" : "font-normal",
          )}
        >
          {thread.title}
        </div>
        <div className="mt-0.5 flex items-center gap-2">
          <span
            className={cn(
              "mono text-[10px] tracking-[0.03em]",
              active ? "text-accent" : "text-ink-3",
            )}
          >
            P{thread.phase} · {phaseLabel}
          </span>
          <span className="text-[11px] text-ink-3">{thread.ago}</span>
        </div>
      </div>
    </button>
  );
}
