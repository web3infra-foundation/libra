"use client";

import { useState, type ReactNode } from "react";

import { IconArrow, IconChev } from "@/components/icons";
import { cn } from "@/lib/utils";

type Tone = "default" | "active" | "muted";

type Props = {
  badge?: string;
  title: string;
  subtitle?: string;
  icon?: ReactNode;
  tone?: Tone;
  defaultOpen?: boolean;
  onClickHead?: () => void;
  selected?: boolean;
  children: ReactNode;
};

const tones: Record<Tone, { bg: string; border: string }> = {
  default: { bg: "bg-paper", border: "border-rule" },
  active: { bg: "bg-paper", border: "border-accent-line" },
  muted: { bg: "bg-paper-2", border: "border-rule" },
};

export function Card({
  badge,
  title,
  subtitle,
  icon,
  tone = "default",
  defaultOpen = true,
  onClickHead,
  selected,
  children,
}: Props) {
  const [open, setOpen] = useState(defaultOpen);
  const t = tones[tone];

  return (
    <div
      className={cn(
        "mb-2.5 overflow-hidden rounded-lg border transition-colors duration-150",
        selected ? "border-accent" : t.border,
        t.bg,
      )}
    >
      <div className="flex items-stretch">
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          className={cn(
            "flex flex-1 items-center justify-between bg-transparent px-3 py-2.5 text-left",
            onClickHead && "border-r border-rule",
          )}
        >
          <div className="flex min-w-0 items-center gap-2">
            {badge && (
              <span className="mono rounded-sm border border-rule-2 bg-paper-2 px-1 py-px text-[9.5px] font-semibold uppercase tracking-[0.04em] text-ink-3">
                {badge}
              </span>
            )}
            {icon && <span className="text-ink-3">{icon}</span>}
            <span className="text-[12.5px] font-semibold">{title}</span>
            {subtitle && (
              <span className="mono text-[10.5px] text-ink-3">{subtitle}</span>
            )}
          </div>
          <div
            className={cn(
              "text-ink-3 transition-transform duration-150",
              open ? "rotate-90" : "rotate-0",
            )}
          >
            <IconChev size={14} />
          </div>
        </button>
        {onClickHead && (
          <button
            type="button"
            onClick={onClickHead}
            className="grid w-9 place-items-center text-ink-3"
            title="Open details"
          >
            <IconArrow size={12} />
          </button>
        )}
      </div>
      {open && (
        <div className="border-t border-rule px-3.5 py-3">{children}</div>
      )}
    </div>
  );
}
