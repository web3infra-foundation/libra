import Link from "next/link";
import type { ReactNode } from "react";
import type { SiteWire } from "@/lib/wire-types";
import { cn, shortRevision } from "@/lib/utils";

type SiteShellProps = {
  readonly site: SiteWire;
  readonly activeNav: "publish" | "code" | "refs" | "ai" | "status";
  readonly children: ReactNode;
};

export function SiteShell({ site, activeNav, children }: SiteShellProps) {
  const navItems: ReadonlyArray<{ key: SiteShellProps["activeNav"]; label: string; href: string }> = [
    { key: "publish", label: "Publish", href: `/sites/${site.slug}/publish` },
    { key: "code", label: "Code", href: `/sites/${site.slug}` },
    { key: "refs", label: "Refs", href: `/sites/${site.slug}/refs` },
    { key: "ai", label: "AI Object Model", href: `/sites/${site.slug}/ai` },
    { key: "status", label: "Status", href: `/sites/${site.slug}/status` },
  ];
  return (
    <div className="min-h-dvh">
      <header className="border-b" style={{ borderColor: "var(--line)" }}>
        <div className="mx-auto flex max-w-6xl flex-wrap items-baseline gap-x-6 gap-y-3 px-6 py-5">
          <div className="flex items-baseline gap-3">
            <Link href="/" className="libra-pill libra-pill-accent">
              libra publish
            </Link>
            <span className="libra-text-faint libra-mono">/</span>
            <Link
              href={`/sites/${site.slug}`}
              className="text-base font-semibold tracking-tight"
            >
              {site.name}
            </Link>
            {site.visibility === "private" ? (
              <span className="libra-pill libra-pill-warn" title="Cloudflare Access required">
                private
              </span>
            ) : (
              <span className="libra-pill" title="public read-only">
                public
              </span>
            )}
            {site.status === "disabled" && (
              <span className="libra-pill libra-pill-bad">disabled</span>
            )}
          </div>
          <nav className="ml-auto flex flex-wrap items-center gap-1">
            {navItems.map((item) => (
              <Link
                key={item.key}
                href={item.href}
                className={cn(
                  "rounded-md px-3 py-1.5 text-sm",
                  activeNav === item.key
                    ? "bg-[var(--surface-3)] text-[var(--ink)]"
                    : "text-[var(--ink-muted)] hover:bg-[var(--surface-2)] hover:text-[var(--ink)]",
                )}
              >
                {item.label}
              </Link>
            ))}
          </nav>
        </div>
        <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-3 px-6 pb-4 text-xs libra-text-muted">
          <span className="libra-mono">slug:</span>
          <span className="libra-mono">{site.slug}</span>
          <span aria-hidden>·</span>
          <span className="libra-mono">repo:</span>
          <span className="libra-mono">{shortRevision(site.repoId)}</span>
          <span aria-hidden>·</span>
          <span className="libra-mono">latest:</span>
          <span className="libra-mono">{shortRevision(site.latestRevisionOid)}</span>
          <span aria-hidden>·</span>
          <span className="libra-mono">refs gen:</span>
          <span className="libra-mono">#{site.refsGeneration}</span>
        </div>
      </header>
      <main className="mx-auto max-w-6xl px-6 py-8">{children}</main>
      <footer className="mx-auto max-w-6xl px-6 pb-12 text-xs libra-text-faint">
        Read-only Libra publish. Restore the repository locally with{" "}
        <code className="libra-mono">
          libra clone libra+cloud://{site.cloneDomain}/{site.slug}
        </code>
        .
      </footer>
    </div>
  );
}
