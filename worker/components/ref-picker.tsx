"use client";

import { useEffect, useMemo, useState } from "react";
import { useRouter, useSearchParams, usePathname } from "next/navigation";
import type { RefWire } from "@/lib/wire-types";
import { cn } from "@/lib/utils";

type RefPickerProps = {
  readonly slug: string;
  readonly refs: readonly RefWire[];
  readonly active: string | null;
};

export function RefPicker({ refs, active }: RefPickerProps) {
  const router = useRouter();
  const pathname = usePathname();
  const searchParams = useSearchParams();
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");

  const branches = useMemo(() => refs.filter((r) => r.refType === "branch"), [refs]);
  const tags = useMemo(() => refs.filter((r) => r.refType === "tag"), [refs]);

  // Match against full or short name; case-insensitive substring.
  const lower = filter.trim().toLowerCase();
  const visibleBranches = lower
    ? branches.filter(
        (r) => r.shortName.toLowerCase().includes(lower) || r.refName.toLowerCase().includes(lower),
      )
    : branches;
  const visibleTags = lower
    ? tags.filter(
        (r) => r.shortName.toLowerCase().includes(lower) || r.refName.toLowerCase().includes(lower),
      )
    : tags;

  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open]);

  const select = (ref: RefWire) => {
    const params = new URLSearchParams(searchParams.toString());
    params.set("ref", ref.refName);
    params.delete("revision");
    router.replace(`${pathname}?${params.toString()}`);
    setOpen(false);
  };

  const activeLabel = active ? prettyRefName(active) : "(default)";

  return (
    <div className="relative inline-block">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className={cn(
          "inline-flex items-center gap-2 rounded-md border px-3 py-1.5 text-sm",
          "bg-[var(--surface)] border-[var(--line)] hover:bg-[var(--surface-2)]",
        )}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <span className="libra-text-muted text-xs">ref</span>
        <span className="libra-mono">{activeLabel}</span>
        <span aria-hidden>▾</span>
      </button>
      {open && (
        <div
          role="dialog"
          className="absolute z-20 mt-2 w-80 max-w-[90vw] rounded-md border bg-[var(--surface)] p-2 shadow-lg"
          style={{ borderColor: "var(--line)" }}
        >
          <input
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder="Filter refs…"
            className="mb-2 w-full rounded-md border bg-[var(--surface-2)] px-2 py-1 text-sm libra-mono"
            style={{ borderColor: "var(--line)" }}
            autoFocus
          />
          <RefSection title={`Branches · ${visibleBranches.length}`} refs={visibleBranches} active={active} onSelect={select} />
          <RefSection title={`Tags · ${visibleTags.length}`} refs={visibleTags} active={active} onSelect={select} />
          {visibleBranches.length === 0 && visibleTags.length === 0 && (
            <p className="px-2 py-3 text-xs libra-text-faint">no refs match</p>
          )}
        </div>
      )}
    </div>
  );
}

function RefSection({
  title,
  refs,
  active,
  onSelect,
}: {
  readonly title: string;
  readonly refs: readonly RefWire[];
  readonly active: string | null;
  readonly onSelect: (ref: RefWire) => void;
}) {
  if (refs.length === 0) return null;
  return (
    <div className="mb-2 last:mb-0">
      <p className="px-2 pb-1 text-[11px] uppercase tracking-wide libra-text-faint">{title}</p>
      <ul className="max-h-60 overflow-auto">
        {refs.map((ref) => (
          <li key={ref.refName}>
            <button
              type="button"
              onClick={() => onSelect(ref)}
              className={cn(
                "flex w-full items-center justify-between gap-3 rounded-sm px-2 py-1 text-left text-sm",
                "hover:bg-[var(--surface-2)]",
                ref.refName === active ? "bg-[var(--surface-3)]" : "",
              )}
            >
              <span className="libra-mono truncate">{ref.shortName}</span>
              <span className="libra-mono text-xs libra-text-faint truncate">
                {ref.revisionOid.slice(0, 12)}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function prettyRefName(refName: string): string {
  if (refName.startsWith("refs/heads/")) return refName.slice("refs/heads/".length);
  if (refName.startsWith("refs/tags/")) return `tag:${refName.slice("refs/tags/".length)}`;
  return refName;
}
