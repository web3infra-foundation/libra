"use client";

import { useEffect, useMemo, useState } from "react";
import {
  ApiError,
  fetchAiObject,
  fetchAiObjects,
  fetchAiVersions,
  type AiObjectsList,
  type AiObjectDetail,
  type AiVersionsList,
} from "@/lib/client/api";
import { cn, formatDate } from "@/lib/utils";

type Props = {
  readonly slug: string;
  readonly refName: string;
};

const LAYERS: ReadonlyArray<{ value: "snapshot" | "event" | "projection"; label: string }> = [
  { value: "snapshot", label: "Snapshot" },
  { value: "event", label: "Event" },
  { value: "projection", label: "Projection" },
];

const COMMON_TYPES = [
  "Intent",
  "Plan",
  "Task",
  "Run",
  "PatchSet",
  "ContextSnapshot",
  "Provenance",
  "IntentEvent",
  "TaskEvent",
  "RunEvent",
  "PlanStepEvent",
  "RunUsage",
  "ToolInvocation",
  "Evidence",
  "Decision",
  "ContextFrame",
  "Thread",
  "Scheduler",
  "QueryIndex",
  "LiveContextWindow",
  "ReadyQueue",
  "ParallelGroup",
  "Checkpoint",
  "RetryRoute",
  "UiCurrentView",
] as const;

export function AiBrowser({ slug, refName }: Props) {
  const [layer, setLayer] = useState<"snapshot" | "event" | "projection" | null>(null);
  const [type, setType] = useState<string | null>(null);
  const [objects, setObjects] = useState<AiObjectsList | null>(null);
  const [versions, setVersions] = useState<AiVersionsList | null>(null);
  const [selected, setSelected] = useState<{ objectType: string; objectId: string } | null>(null);
  const [detail, setDetail] = useState<AiObjectDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadingObjects, setLoadingObjects] = useState(false);
  const [loadingDetail, setLoadingDetail] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoadingObjects(true);
    setError(null);
    Promise.all([
      fetchAiObjects(slug, { ref: refName, type: type ?? undefined, layer: layer ?? undefined, limit: 200 }),
      fetchAiVersions(slug, { ref: refName }),
    ])
      .then(([objs, vers]) => {
        if (cancelled) return;
        setObjects(objs);
        setVersions(vers);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof ApiError ? err.message : "failed to load AI objects");
      })
      .finally(() => {
        if (cancelled) return;
        setLoadingObjects(false);
      });
    return () => {
      cancelled = true;
    };
  }, [slug, refName, type, layer]);

  useEffect(() => {
    if (!selected) {
      setDetail(null);
      return;
    }
    let cancelled = false;
    setLoadingDetail(true);
    fetchAiObject(slug, selected.objectType, selected.objectId, { ref: refName })
      .then((d) => {
        if (cancelled) return;
        setDetail(d);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof ApiError ? err.message : "failed to load AI object detail");
      })
      .finally(() => {
        if (cancelled) return;
        setLoadingDetail(false);
      });
    return () => {
      cancelled = true;
    };
  }, [slug, refName, selected]);

  const objectGroupsByType = useMemo(() => {
    if (!objects) return new Map<string, number>();
    const groups = new Map<string, number>();
    for (const obj of objects.objects) {
      groups.set(obj.objectType, (groups.get(obj.objectType) ?? 0) + 1);
    }
    return groups;
  }, [objects]);

  return (
    <div className="grid gap-6 md:grid-cols-[260px_minmax(0,1fr)]">
      <aside className="space-y-4">
        <Section title="Layer">
          <div className="flex flex-wrap gap-1">
            <FilterChip active={layer === null} onClick={() => setLayer(null)}>
              all
            </FilterChip>
            {LAYERS.map((entry) => (
              <FilterChip
                key={entry.value}
                active={layer === entry.value}
                onClick={() => setLayer(layer === entry.value ? null : entry.value)}
              >
                {entry.label}
              </FilterChip>
            ))}
          </div>
        </Section>
        <Section title="Object type">
          <div className="flex flex-wrap gap-1">
            <FilterChip active={type === null} onClick={() => setType(null)}>
              any
            </FilterChip>
            {COMMON_TYPES.map((entry) => {
              const count = objectGroupsByType.get(entry) ?? 0;
              return (
                <FilterChip
                  key={entry}
                  active={type === entry}
                  onClick={() => setType(type === entry ? null : entry)}
                  trailing={count > 0 ? String(count) : undefined}
                >
                  {entry}
                </FilterChip>
              );
            })}
          </div>
        </Section>
        <Section title="Bundles">
          {versions && versions.versions.length > 0 ? (
            <ul className="space-y-1">
              {versions.versions.map((entry) => (
                <li key={entry.aiVersionId} className="text-xs">
                  <p className="libra-mono truncate">{entry.aiVersionId}</p>
                  <p className="libra-text-faint">
                    {entry.objectCount.toLocaleString()} objects · redaction {entry.redactionMode}
                  </p>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-xs libra-text-muted">No bundles in this revision.</p>
          )}
        </Section>
      </aside>

      <section className="space-y-4">
        {error && (
          <div className="libra-card libra-card-pad text-sm text-[var(--bad)]">{error}</div>
        )}
        <div className="grid gap-4 md:grid-cols-2">
          <ObjectList
            objects={objects?.objects ?? []}
            loading={loadingObjects}
            selected={selected}
            onSelect={(item) => setSelected(item)}
          />
          <ObjectDetail detail={detail} loading={loadingDetail} />
        </div>
      </section>
    </div>
  );
}

function Section({ title, children }: { readonly title: string; readonly children: React.ReactNode }) {
  return (
    <div>
      <p className="mb-2 text-[11px] uppercase tracking-wide libra-text-faint">{title}</p>
      {children}
    </div>
  );
}

function FilterChip({
  active,
  onClick,
  trailing,
  children,
}: {
  readonly active: boolean;
  readonly onClick: () => void;
  readonly trailing?: string;
  readonly children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "libra-pill",
        active ? "libra-pill-accent" : "",
        "cursor-pointer",
      )}
    >
      {children}
      {trailing ? <span className="libra-text-faint">· {trailing}</span> : null}
    </button>
  );
}

function ObjectList({
  objects,
  loading,
  selected,
  onSelect,
}: {
  readonly objects: ReadonlyArray<{ readonly objectType: string; readonly objectId: string; readonly layer: string; readonly payloadSha256: string; readonly createdAt: string; readonly redactionMode: string }>;
  readonly loading: boolean;
  readonly selected: { readonly objectType: string; readonly objectId: string } | null;
  readonly onSelect: (item: { readonly objectType: string; readonly objectId: string }) => void;
}) {
  if (loading && objects.length === 0) {
    return (
      <div className="libra-card libra-card-pad text-sm libra-text-muted">Loading objects…</div>
    );
  }
  if (objects.length === 0) {
    return (
      <div className="libra-card libra-card-pad text-sm libra-text-muted">
        No AI objects match the current filters.
      </div>
    );
  }
  return (
    <div className="libra-card max-h-[60vh] overflow-auto">
      <ul>
        {objects.map((entry) => {
          const isSelected =
            selected !== null &&
            selected.objectType === entry.objectType &&
            selected.objectId === entry.objectId;
          return (
            <li key={`${entry.objectType}::${entry.objectId}`}>
              <button
                type="button"
                onClick={() => onSelect({ objectType: entry.objectType, objectId: entry.objectId })}
                className={cn(
                  "flex w-full flex-col items-start gap-0.5 border-b px-4 py-2 text-left text-sm",
                  isSelected ? "bg-[var(--surface-3)]" : "hover:bg-[var(--surface-2)]",
                )}
                style={{ borderColor: "var(--line)" }}
              >
                <span className="flex w-full items-baseline justify-between gap-3">
                  <span className="libra-mono font-medium">{entry.objectType}</span>
                  <span className="libra-pill text-[10px]">{entry.layer}</span>
                </span>
                <span className="libra-mono text-xs libra-text-muted truncate">{entry.objectId}</span>
                <span className="text-[11px] libra-text-faint">
                  {formatDate(entry.createdAt)} · sha {entry.payloadSha256.slice(0, 8)} · redaction{" "}
                  {entry.redactionMode}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function ObjectDetail({
  detail,
  loading,
}: {
  readonly detail: AiObjectDetail | null;
  readonly loading: boolean;
}) {
  if (loading && !detail) {
    return (
      <div className="libra-card libra-card-pad text-sm libra-text-muted">Loading detail…</div>
    );
  }
  if (!detail) {
    return (
      <div className="libra-card libra-card-pad text-sm libra-text-muted">
        Select an object to inspect its payload.
      </div>
    );
  }
  return (
    <div className="libra-card libra-card-pad space-y-3">
      <header>
        <p className="libra-mono text-sm font-semibold">
          {detail.index.objectType} <span className="libra-text-faint">·</span>{" "}
          <span className="libra-mono">{detail.index.objectId}</span>
        </p>
        <p className="text-xs libra-text-muted">
          layer {detail.index.layer} · redaction {detail.index.redactionMode} · sha{" "}
          {detail.index.payloadSha256.slice(0, 12)}
        </p>
      </header>
      <pre className="libra-codebox max-h-[55vh] overflow-auto text-xs">
        <code>{JSON.stringify(detail.payload, null, 2)}</code>
      </pre>
    </div>
  );
}
