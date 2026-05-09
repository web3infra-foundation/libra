import Link from "next/link";
import { ClonePanel, type CloneVariant } from "@/components/clone-panel";
import { EmptyState } from "@/components/empty-state";
import { SiteShell } from "@/components/site-shell";
import {
  loadPublishOverviewForSite,
  loadSiteContextForSlug,
} from "@/lib/server/page-helpers";
import { cn, formatDate, shortRevision } from "@/lib/utils";
import type { PublishOverviewRefWire } from "@/lib/wire-types";

export const runtime = "edge";
export const dynamic = "force-dynamic";

type Props = {
  readonly params: Promise<{ readonly slug: string }>;
  readonly searchParams: Promise<Record<string, string | string[] | undefined>>;
};

const REF_FILTERS = ["all", "branch", "tag"] as const;
type RefFilter = (typeof REF_FILTERS)[number];

function readFilter(raw: string | string[] | undefined): RefFilter {
  const value = typeof raw === "string" ? raw : null;
  return REF_FILTERS.find((f) => f === value) ?? "all";
}

function readRefSelection(
  raw: string | string[] | undefined,
  refs: readonly PublishOverviewRefWire[],
  fallbackName: string | null,
): PublishOverviewRefWire | null {
  if (typeof raw === "string") {
    const exact = refs.find((r) => r.refName === raw);
    if (exact) return exact;
    const byShort = refs.find((r) => r.shortName === raw);
    if (byShort) return byShort;
  }
  if (fallbackName) {
    const fallback = refs.find((r) => r.refName === fallbackName);
    if (fallback) return fallback;
  }
  return refs.find((r) => r.isDefault) ?? refs[0] ?? null;
}

/**
 * POSIX shell single-quote: wrap the value in `'...'` and replace any
 * embedded single-quote with `'\''`. Safe for any printable input;
 * we use it on every dynamic URL or oid that lands inside the
 * copy-pasted clone command so a hostile or unusual ref name cannot
 * inject extra arguments.
 */
function shellQuote(value: string): string {
  return `'${value.replace(/'/g, "'\\''")}'`;
}

function buildCloneVariants(args: {
  readonly cloneDomain: string;
  readonly slug: string;
  readonly repoId: string;
  readonly defaultRefShort: string | null;
  readonly defaultTargetOid: string | null;
  readonly selected: PublishOverviewRefWire | null;
}): readonly CloneVariant[] {
  const base = `libra+cloud://${args.cloneDomain}/${args.slug}`;
  const stable = `libra+cloud://${args.cloneDomain}/repo/${args.repoId}`;
  const sel = args.selected;
  return [
    {
      id: "default",
      title: "Default — clone HEAD of default branch",
      command: `libra clone ${shellQuote(base)}`,
      notes: args.defaultRefShort && args.defaultTargetOid
        ? `Resolves to refs/heads/${args.defaultRefShort} @ ${shortRevision(args.defaultTargetOid)}`
        : "Resolves to the site's default ref at clone time.",
    },
    {
      id: "ref",
      title: "Pin to a branch or tag",
      command: sel
        ? `libra clone ${shellQuote(`${base}?${new URLSearchParams({ ref: sel.refName }).toString()}`)}`
        : `libra clone "${base}?ref=<branch-or-tag>"`,
      notes: sel
        ? `${sel.refType === "tag" ? "Tag" : "Branch"} · ${sel.refName} @ ${shortRevision(sel.targetOid)}`
        : "Use the full ref name (refs/heads/... or refs/tags/...).",
    },
    {
      id: "revision",
      title: "Pin to an immutable revision",
      // Clone with the FULL revision oid — `shortRevision` is only
      // for human-readable labels. A short oid is ambiguous and the
      // CLI's revision parser rejects truncated values.
      command: sel
        ? `libra clone ${shellQuote(`${base}?${new URLSearchParams({ revision: sel.revisionOid }).toString()}`)}`
        : `libra clone "${base}?revision=<oid>"`,
      notes: sel
        ? `Frozen at oid ${shortRevision(sel.revisionOid)}. Use a full commit oid; the keyword latest resolves at clone time.`
        : "Use a full commit oid; the keyword latest resolves at clone time.",
    },
    {
      id: "stable",
      title: "Stable repo URL — survives slug rename",
      command: `libra clone ${shellQuote(stable)}`,
      notes: "Domain-qualified by repo_id; survives a slug rename.",
    },
  ];
}

export default async function PublishHeroPage({ params, searchParams }: Props) {
  const { slug } = await params;
  const sp = await searchParams;
  const ctx = await loadSiteContextForSlug(slug);
  const overview = await loadPublishOverviewForSite(ctx);
  const refs = overview.refs;
  const filter = readFilter(sp.filter);
  const filteredRefs = filter === "all" ? refs : refs.filter((r) => r.refType === filter);
  const counts = {
    all: refs.length,
    branch: refs.filter((r) => r.refType === "branch").length,
    tag: refs.filter((r) => r.refType === "tag").length,
  };

  const defaultRef = overview.defaultRef;
  const selected = readRefSelection(sp.ref, refs, defaultRef?.refName ?? null);
  const cloneVariants = buildCloneVariants({
    cloneDomain: ctx.siteWire.cloneDomain,
    slug: ctx.siteWire.slug,
    repoId: ctx.siteWire.repoId,
    defaultRefShort: defaultRef?.shortName ?? null,
    defaultTargetOid: defaultRef?.targetOid ?? null,
    selected,
  });

  if (refs.length === 0) {
    return (
      <SiteShell site={ctx.siteWire} activeNav="publish">
        <Header
          slug={ctx.siteWire.slug}
          name={ctx.siteWire.name}
          defaultRefShort={null}
          defaultRevisionOid={ctx.siteWire.latestRevisionOid}
        />
        <div className="mt-8">
          <EmptyState
            title="No refs published yet"
            description="Run `libra publish sync` from the local repository to publish refs/heads/* and refs/tags/*."
            hint={
              <span>
                Once published, this page surfaces clone commands and the
                publish health of every branch and tag.
              </span>
            }
          />
        </div>
      </SiteShell>
    );
  }

  return (
    <SiteShell site={ctx.siteWire} activeNav="publish">
      <Header
        slug={ctx.siteWire.slug}
        name={ctx.siteWire.name}
        defaultRefShort={defaultRef?.shortName ?? null}
        defaultRevisionOid={defaultRef?.revisionOid ?? ctx.siteWire.latestRevisionOid}
      />

      <div className="mt-6 space-y-6">
        <ClonePanel
          selectedRefName={selected?.refName ?? defaultRef?.refName ?? "—"}
          selectedRevisionOid={selected?.revisionOid ?? null}
          variants={cloneVariants}
        />

        <SelectedRefFacts selected={selected} />

        <QuickTaskGrid slug={ctx.siteWire.slug} refName={selected?.refName ?? null} />

        <RefsTable
          slug={ctx.siteWire.slug}
          refs={filteredRefs}
          allRefs={refs}
          activeRefName={selected?.refName ?? null}
          filter={filter}
          counts={counts}
        />
      </div>
    </SiteShell>
  );
}

function SelectedRefFacts({
  selected,
}: {
  readonly selected: PublishOverviewRefWire | null;
}) {
  if (!selected) return null;
  return (
    <section
      aria-label="Selected ref"
      className="rounded-md p-4"
      style={{
        background: "var(--paper)",
        border: "1px solid var(--paper-line)",
      }}
    >
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <Fact label="HEAD oid" value={shortRevision(selected.targetOid)} mono />
        <Fact label="Selected ref" value={selected.refName} mono />
        <Fact
          label="Last published"
          value={formatDate(selected.revisionCreatedAt ?? selected.updatedAt)}
        />
        <Fact
          label="Publish state"
          value={<PublishStatePill state={selected.publishState} />}
        />
      </div>
    </section>
  );
}

function Fact({
  label,
  value,
  mono,
}: {
  readonly label: string;
  readonly value: React.ReactNode;
  readonly mono?: boolean;
}) {
  return (
    <div className="min-w-0">
      <div className="lb-eyebrow mb-1">{label}</div>
      <div
        className={cn("text-[13px] truncate", mono ? "lb-mono" : "")}
        style={{ color: "var(--ink-deep)" }}
      >
        {value}
      </div>
    </div>
  );
}

function Header({
  slug,
  name,
  defaultRefShort,
  defaultRevisionOid,
}: {
  readonly slug: string;
  readonly name: string;
  readonly defaultRefShort: string | null;
  readonly defaultRevisionOid: string | null;
}) {
  return (
    <div>
      <p className="lb-eyebrow">Publish</p>
      <h1 className="lb-h1 mt-1">{name || slug}</h1>
      <p className="lb-meta mt-2">
        Recovery & clone entry point. Default ref{" "}
        <span className="lb-mono">{defaultRefShort ?? "—"}</span>
        {defaultRevisionOid ? (
          <>
            {" "}@ <span className="lb-mono">{shortRevision(defaultRevisionOid)}</span>
          </>
        ) : null}
        .
      </p>
    </div>
  );
}

function QuickTaskGrid({
  slug,
  refName,
}: {
  readonly slug: string;
  readonly refName: string | null;
}) {
  const refQuery = refName ? `?ref=${encodeURIComponent(refName)}` : "";
  const items: ReadonlyArray<{
    readonly eyebrow: string;
    readonly title: string;
    readonly desc: string;
    readonly href: string;
  }> = [
    {
      eyebrow: "Browse",
      title: "Code & directory tree",
      desc: "Explore files, language stats, redacted regions.",
      href: `/sites/${slug}${refQuery}`,
    },
    {
      eyebrow: "Inspect",
      title: "AI object model",
      desc: "Snapshots, events, and Libra projections.",
      href: `/sites/${slug}/ai${refQuery}`,
    },
    {
      eyebrow: "Track",
      title: "Refs & publish status",
      desc: "All branches and tags · publish health.",
      // Codex pass-1 deviation note: keep the Track action on the
      // publish route so the design's single-page hero stays intact.
      // The query carries the selected ref (so the table highlights
      // the same row); the hash scrolls to the table heading.
      href: `/sites/${slug}/publish${refQuery}#refs-table-heading`,
    },
  ];
  return (
    <section aria-label="Quick tasks">
      <h2 className="lb-h2 mb-3">What do you need to do?</h2>
      <div className="grid gap-3.5 sm:grid-cols-2 lg:grid-cols-3">
        {items.map((it) => (
          <Link
            key={it.eyebrow}
            href={it.href}
            className="group flex flex-col gap-1.5 rounded-md p-4"
            style={{
              background: "var(--paper)",
              border: "1px solid var(--paper-line)",
            }}
          >
            <span className="lb-eyebrow">{it.eyebrow}</span>
            <span className="lb-h2 text-[15px]">{it.title}</span>
            <span className="lb-meta text-[12.5px]">{it.desc}</span>
            <span
              className="mt-2 inline-flex items-center gap-1 text-[12px] font-semibold"
              style={{ color: "var(--gold)" }}
            >
              Open <span aria-hidden>→</span>
            </span>
          </Link>
        ))}
      </div>
    </section>
  );
}

function RefsTable({
  slug,
  refs,
  allRefs,
  activeRefName,
  filter,
  counts,
}: {
  readonly slug: string;
  readonly refs: readonly PublishOverviewRefWire[];
  readonly allRefs: readonly PublishOverviewRefWire[];
  readonly activeRefName: string | null;
  readonly filter: RefFilter;
  readonly counts: { readonly all: number; readonly branch: number; readonly tag: number };
}) {
  return (
    <section
      aria-labelledby="refs-table-heading"
      className="overflow-hidden rounded-md"
      style={{
        background: "var(--paper)",
        border: "1px solid var(--paper-line)",
      }}
    >
      <header
        className="flex flex-wrap items-center justify-between gap-3 border-b px-4 py-3"
        style={{ borderColor: "var(--paper-line)" }}
      >
        <div>
          <p className="lb-eyebrow">All refs</p>
          <h2 id="refs-table-heading" className="lb-h2 text-[16px]">
            Branches & tags · publish status
          </h2>
        </div>
        {/*
          Codex pass-2 P2: this control is link-based navigation
          (each filter is a real URL the user can bookmark). The
          ARIA tab pattern requires roving-focus keyboard handling
          we can't easily provide on `<a>` elements. Render as a
          plain nav so the browser's default Tab traversal works.
        */}
        <nav
          aria-label="Ref filter"
          className="flex overflow-hidden rounded"
          style={{ border: "1px solid var(--paper-line)" }}
        >
          {REF_FILTERS.map((id) => {
            const on = filter === id;
            return (
              <Link
                key={id}
                aria-current={on ? "page" : undefined}
                href={
                  id === "all"
                    ? `/sites/${slug}/publish${activeRefName ? `?ref=${encodeURIComponent(activeRefName)}` : ""}`
                    : `/sites/${slug}/publish?filter=${id}${activeRefName ? `&ref=${encodeURIComponent(activeRefName)}` : ""}`
                }
                className="px-3 py-1.5 text-[11.5px] font-semibold"
                style={{
                  background: on ? "var(--ink-deep)" : "var(--paper)",
                  color: on ? "var(--paper)" : "var(--ink-mid)",
                }}
              >
                {id === "all" ? "All" : id === "branch" ? "Branches" : "Tags"}{" "}
                <span className="lb-mono opacity-70">{counts[id]}</span>
              </Link>
            );
          })}
        </nav>
      </header>

      <div className="overflow-x-auto">
        <table
          className="w-full min-w-[760px] border-collapse text-left"
          style={{ fontFamily: "var(--font-sans)" }}
        >
          <thead>
            <tr>
              <RefHeaderCell>Name</RefHeaderCell>
              <RefHeaderCell>Type</RefHeaderCell>
              <RefHeaderCell>Target oid</RefHeaderCell>
              <RefHeaderCell>Revision oid</RefHeaderCell>
              <RefHeaderCell>Files</RefHeaderCell>
              <RefHeaderCell>Last published</RefHeaderCell>
              <RefHeaderCell>Publish</RefHeaderCell>
              <RefHeaderCell align="right">AI versions</RefHeaderCell>
            </tr>
          </thead>
          <tbody>
            {refs.length === 0 ? (
              <tr>
                <td colSpan={8} className="lb-meta px-4 py-6 text-center">
                  No {filter === "all" ? "refs" : filter === "branch" ? "branches" : "tags"} match this filter.
                  {allRefs.length > 0 && (
                    <>
                      {" "}
                      <Link
                        href={`/sites/${slug}/publish`}
                        className="lb-link"
                      >
                        clear filter
                      </Link>
                      .
                    </>
                  )}
                </td>
              </tr>
            ) : (
              refs.map((ref) => (
                <RefRow
                  key={ref.refName}
                  slug={slug}
                  ref={ref}
                  active={ref.refName === activeRefName}
                />
              ))
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function RefHeaderCell({
  children,
  align,
}: {
  readonly children: React.ReactNode;
  readonly align?: "left" | "right";
}) {
  return (
    <th
      scope="col"
      className="lb-eyebrow border-b px-3 py-2.5 text-[10.5px]"
      style={{
        background: "var(--paper-deep)",
        borderColor: "var(--paper-line)",
        textAlign: align ?? "left",
      }}
    >
      {children}
    </th>
  );
}

function RefRow({
  slug,
  ref: row,
  active,
}: {
  readonly slug: string;
  readonly ref: PublishOverviewRefWire;
  readonly active: boolean;
}) {
  const cellStyle = {
    background: active ? "var(--paper-deep)" : "transparent",
    borderColor: "var(--paper-edge)",
  } as const;
  return (
    <tr>
      <td
        className="border-b px-3 py-3"
        style={{
          ...cellStyle,
          borderLeft: active ? "2px solid var(--gold)" : "2px solid transparent",
          minWidth: 0,
        }}
      >
        <div className="flex items-center gap-2">
          <Link
            href={`/sites/${slug}/publish?ref=${encodeURIComponent(row.refName)}`}
            className="lb-mono truncate text-[12.5px]"
            style={{
              color: "var(--ink-deep)",
              fontWeight: active ? 600 : 500,
            }}
          >
            {row.refName}
          </Link>
          {row.isDefault && (
            <span className="lb-chip lb-chip-info" style={{ height: 18 }}>
              default
            </span>
          )}
        </div>
      </td>
      <td className="border-b px-3 py-3" style={cellStyle}>
        <span className="lb-eyebrow text-[10px]">{row.refType}</span>
      </td>
      <td className="border-b px-3 py-3" style={cellStyle}>
        <span className="lb-mono text-[11.5px]" style={{ color: "var(--ink-mid)" }}>
          {shortRevision(row.targetOid)}
        </span>
      </td>
      <td className="border-b px-3 py-3" style={cellStyle}>
        <span className="lb-mono text-[11.5px]" style={{ color: "var(--ink-mid)" }}>
          {shortRevision(row.revisionOid)}
        </span>
      </td>
      <td className="border-b px-3 py-3" style={cellStyle}>
        <span className="lb-mono text-[12px]">
          {row.fileCount.toLocaleString()}
        </span>
      </td>
      <td className="border-b px-3 py-3" style={cellStyle}>
        <span className="lb-mono text-[11.5px]" style={{ color: "var(--ink-mid)" }}>
          {formatDate(row.revisionCreatedAt ?? row.updatedAt)}
        </span>
      </td>
      <td className="border-b px-3 py-3" style={cellStyle}>
        <PublishStatePill state={row.publishState} />
      </td>
      <td className="border-b px-3 py-3 text-right" style={cellStyle}>
        <span
          className="lb-mono text-[12.5px]"
          style={{
            color: row.aiVersionsCount > 0 ? "var(--ink-deep)" : "var(--ink-faint)",
          }}
        >
          {row.aiVersionsCount}
        </span>
      </td>
    </tr>
  );
}

function PublishStatePill({
  state,
}: {
  readonly state: "syncing" | "published" | "failed" | null;
}) {
  if (state === "published") {
    return <span className="lb-chip lb-chip-good">published</span>;
  }
  if (state === "syncing") {
    return <span className="lb-chip lb-chip-warn">syncing</span>;
  }
  if (state === "failed") {
    return <span className="lb-chip lb-chip-bad">failed</span>;
  }
  return <span className="lb-chip">unknown</span>;
}
