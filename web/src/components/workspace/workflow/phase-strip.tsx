"use client";

import { Fragment } from "react";

import { IconCheck } from "@/components/icons";
import { PHASES } from "@/lib/mock";
import { cn } from "@/lib/utils";

type Props = {
  current: number;
};

export function PhaseStrip({ current }: Props) {
  return (
    <div className="mb-1 flex items-start gap-0 px-1 pb-[18px] pt-1">
      {PHASES.map((p, i) => {
        const done = i < current;
        const active = i === current;
        return (
          <Fragment key={p.n}>
            <div className="flex min-w-12 flex-col items-center text-center">
              <div
                className={cn(
                  "mono grid h-[22px] w-[22px] place-items-center rounded-full border text-[10px] font-semibold",
                  done && "border-ink bg-ink text-white",
                  active && "border-accent bg-accent text-white",
                  !done && !active && "border-rule-2 bg-paper text-ink-3",
                )}
              >
                {done ? <IconCheck size={10} sw={2.5} /> : p.n}
              </div>
              <div
                className={cn(
                  "mt-1.5 text-[10.5px] tracking-[-0.01em]",
                  active ? "font-semibold text-ink" : done ? "font-medium text-ink-2" : "font-medium text-ink-3",
                )}
              >
                {p.name}
              </div>
              <div className="mt-px text-[9.5px] text-ink-3">{p.blurb}</div>
            </div>
            {i < PHASES.length - 1 && (
              <div
                className={cn(
                  "mt-2.5 h-px flex-1",
                  done ? "bg-ink" : "bg-rule",
                )}
              />
            )}
          </Fragment>
        );
      })}
    </div>
  );
}
