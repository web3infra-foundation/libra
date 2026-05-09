"use client";

import { useEffect, useMemo, useState } from "react";
import { cn, shortRevision } from "@/lib/utils";

/**
 * Clone command panel — paper-navy port of `_design_reference/routes/publish.jsx`.
 *
 * Renders four immutable command shapes built from server-supplied
 * site identity (clone domain, slug, repo id) and the currently
 * selected ref. Selecting a tab does not refetch data; the
 * server delivers every variant's text up-front so the panel can
 * stay client-side without leaking R2 / D1 internals.
 */
export type CloneVariant = {
  readonly id: "default" | "ref" | "revision" | "stable";
  readonly title: string;
  readonly command: string;
  readonly notes: string;
};

export function ClonePanel({
  selectedRefName,
  selectedRevisionOid,
  variants,
}: {
  readonly selectedRefName: string;
  readonly selectedRevisionOid: string | null;
  readonly variants: readonly CloneVariant[];
}) {
  const [tab, setTab] = useState<CloneVariant["id"]>(
    variants[0]?.id ?? "default",
  );
  // Reset to default when the available variants change (e.g. after
  // the user picks a different ref via the picker on a parent page).
  useEffect(() => {
    if (!variants.find((v) => v.id === tab)) {
      setTab(variants[0]?.id ?? "default");
    }
  }, [variants, tab]);
  const active = useMemo(
    () => variants.find((v) => v.id === tab) ?? variants[0],
    [variants, tab],
  );
  if (!active) return null;

  return (
    <section
      aria-labelledby="clone-panel-heading"
      className="lb-card overflow-hidden"
      style={{ background: "var(--paper)" }}
    >
      <header
        className="flex flex-wrap items-start justify-between gap-4 border-b px-5 py-4"
        style={{ background: "var(--paper)", borderColor: "var(--paper-line)" }}
      >
        <div className="min-w-0 flex-1">
          <p className="lb-eyebrow">Recovery / clone</p>
          <h2 id="clone-panel-heading" className="lb-h2 mt-1 text-[20px]">
            Restore this repository with the Libra CLI
          </h2>
          <p className="lb-meta mt-2 max-w-xl">
            This page exposes publish metadata and the clone command. The
            CLI uses your local Cloudflare/Libra configuration to read the
            published code and AI object model directly from D1 and R2 —
            no Worker download or auth flow runs on this page.
          </p>
        </div>
        <div className="flex flex-col items-end gap-1 text-right">
          <span className="lb-eyebrow">Selected ref</span>
          <span
            className="lb-mono text-[13.5px] font-semibold"
            style={{ color: "var(--ink-deep)" }}
          >
            {selectedRefName}
          </span>
          {selectedRevisionOid && (
            <span
              className="lb-mono text-[11px]"
              style={{ color: "var(--ink-soft)" }}
            >
              {shortRevision(selectedRevisionOid)}
            </span>
          )}
        </div>
      </header>

      <div
        role="tablist"
        aria-label="Clone variant"
        className="flex gap-0 overflow-x-auto border-b px-5"
        style={{ background: "var(--paper)", borderColor: "var(--paper-line)" }}
      >
        {variants.map((v) => {
          const on = v.id === tab;
          return (
            <button
              key={v.id}
              type="button"
              role="tab"
              aria-selected={on}
              onClick={() => setTab(v.id)}
              className={cn(
                "whitespace-nowrap px-4 py-3 text-[12.5px]",
                on ? "font-semibold" : "font-medium",
              )}
              style={{
                color: on ? "var(--ink-deep)" : "var(--ink-soft)",
                borderBottom: `2px solid ${on ? "var(--gold)" : "transparent"}`,
                marginBottom: -1,
              }}
            >
              {v.title}
            </button>
          );
        })}
      </div>

      <div className="px-5 py-4">
        <CommandLine value={active.command} />
        <p className="lb-meta mt-3 text-[12px]">{active.notes}</p>
      </div>
    </section>
  );
}

function CommandLine({ value }: { readonly value: string }) {
  const [copied, setCopied] = useState(false);
  const onCopy = async () => {
    try {
      // navigator.clipboard is undefined in non-HTTPS / older Workers
      // previews; treat the failure as a no-op and never throw.
      await navigator.clipboard?.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };
  return (
    <div
      className="flex flex-wrap items-stretch overflow-hidden rounded-md"
      style={{ background: "var(--paper)", border: "1px solid var(--ink)" }}
    >
      <pre
        className="lb-mono m-0 min-w-0 flex-1 overflow-x-auto px-4 py-3 text-[13.5px]"
        style={{ color: "var(--ink-deep)", whiteSpace: "pre" }}
      >
        <span style={{ color: "var(--ink-faint)", userSelect: "none" }}>$ </span>
        {value}
      </pre>
      <div
        className="flex items-center justify-end px-2 py-2"
        style={{
          background: "var(--paper-deep)",
          borderLeft: "1px solid var(--paper-line)",
        }}
      >
        <button
          type="button"
          onClick={onCopy}
          aria-live="polite"
          className="rounded px-2 py-1 text-[12px] font-medium"
          style={{
            color: copied ? "var(--good)" : "var(--ink-mid)",
            background: "transparent",
          }}
        >
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
    </div>
  );
}
