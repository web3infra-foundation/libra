import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

import type { CompactChange, FeaturedChange } from "./thread-data";
import styles from "./thread-page.module.css";

const compactToneClassNames = {
  review: {
    article: "opacity-80 hover:opacity-100",
    title: "text-black/50 group-hover:text-black/70",
    status: "text-[var(--change-blue)]",
  },
  merged: {
    article: "opacity-60 hover:opacity-100",
    title: "text-black/40 group-hover:text-black/60",
    status: "text-[var(--change-purple)]",
  },
  archived: {
    article: "opacity-40 hover:opacity-80",
    title: "text-black/40 group-hover:text-black/55",
    status: "text-black/45",
  },
} as const;

export function FeaturedChangeCard({ change }: { change: FeaturedChange }) {
  return (
    <article className={cn("relative", styles.featuredArticle)}>
      <div
        aria-hidden
        className={cn(
          "absolute -left-4 top-2 hidden h-full w-1 rounded-full bg-black/15 md:block lg:-left-6",
          styles.featureRail,
        )}
      />

      <header className="mb-6">
        <div className="mb-2 flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <h2
            className={cn(
              "cursor-default text-[clamp(3.8rem,8vw,5.8rem)] font-light tracking-[-0.09em] text-black",
              styles.glitchHover,
            )}
          >
            {change.id}
          </h2>

          <div className="text-left font-mono text-xs text-black/60 lg:text-right">
            <p className="mb-1">
              LAST UPDATED: <span className="text-black">{change.updatedAt}</span>
            </p>
            <p>
              STATUS:{" "}
              <span className="text-[var(--change-green)]">{change.status}</span>
            </p>
          </div>
        </div>

        <Badge
          variant="outline"
          className="rounded-md border-emerald-950/20 bg-emerald-950/10 px-2 py-1 font-mono text-xs font-normal text-emerald-700"
        >
          {`> ${change.filePath}`}
        </Badge>
      </header>

      <div className="grid gap-10 xl:grid-cols-12 xl:gap-12">
        <div className="xl:col-span-8">
          <h3 className="mb-4 text-xl font-medium text-black/80">
            {change.title}
          </h3>
          <p className="max-w-3xl font-mono text-sm leading-relaxed text-black/60">
            {change.description}
          </p>
        </div>

        <div className="space-y-6 xl:col-span-4">
          <section className="border-t border-black/10 pt-4">
            <h3 className="mb-2 font-mono text-[10px] uppercase tracking-[0.22em] text-black/45">
              Author
            </h3>
            <div className="flex items-center gap-3">
              <Avatar className="size-6 after:border-white/10">
                <AvatarFallback className="bg-[linear-gradient(135deg,#3b0764,#1d4ed8)] text-[10px] font-semibold text-white">
                  {change.author.initials}
                </AvatarFallback>
              </Avatar>
              <span className="text-sm text-black">{change.author.name}</span>
            </div>
          </section>

          <section className="border-t border-black/10 pt-4">
            <h3 className="mb-2 font-mono text-[10px] uppercase tracking-[0.22em] text-black/45">
              Impact Analysis
            </h3>
            <div className="flex flex-col gap-1">
              {change.impact.map((metric) => (
                <div
                  key={metric.label}
                  className="flex items-center justify-between font-mono text-sm text-black/60"
                >
                  <span>{metric.label}</span>
                  <span
                    className={cn(
                      "text-black",
                      metric.tone === "positive" &&
                        "text-[var(--change-green)]",
                    )}
                  >
                    {metric.value}
                  </span>
                </div>
              ))}
            </div>
          </section>

          <section className="border-t border-black/10 pt-4">
            <h3 className="mb-2 font-mono text-[10px] uppercase tracking-[0.22em] text-black/45">
              Discussion
            </h3>
            <p className="font-mono text-sm text-black/60">
              <span className="text-black">{change.comments}</span> from Reviewers
            </p>
          </section>
        </div>
      </div>
    </article>
  );
}

export function CompactChangeCard({ change }: { change: CompactChange }) {
  const toneClassNames = compactToneClassNames[change.tone];

  return (
    <article
      className={cn(
        "group relative transition-opacity duration-200",
        toneClassNames.article,
      )}
    >
      <header className="mb-6">
        <div className="mb-2 flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <h2
            className={cn(
              "text-[clamp(3.4rem,7vw,5.2rem)] font-light tracking-[-0.09em] transition-colors",
              toneClassNames.title,
            )}
          >
            {change.id}
          </h2>

          <div className="text-left font-mono text-xs text-black/60 lg:text-right">
            <p className="mb-1">
              LAST UPDATED:{" "}
              <span className="text-black/70">{change.updatedAt}</span>
            </p>
            <p>
              STATUS: <span className={toneClassNames.status}>{change.status}</span>
            </p>
          </div>
        </div>
      </header>

      <div className="grid gap-10 xl:grid-cols-12 xl:gap-12">
        <div className="xl:col-span-8">
          <h3 className="mb-4 text-xl font-medium text-black/70">
            {change.title}
          </h3>
          <p className="max-w-3xl font-mono text-sm leading-relaxed text-black/60">
            {change.description}
          </p>
        </div>

        <div
          className={cn(
            "grid gap-4 xl:col-span-4",
            change.meta.length > 1 ? "sm:grid-cols-2" : "sm:grid-cols-1",
          )}
        >
          {change.meta.map((item) => (
            <div key={`${change.id}-${item.label}`}>
              <h3 className="mb-1 font-mono text-[10px] uppercase tracking-[0.22em] text-black/70">
                {item.label}
              </h3>
              <p className="text-sm text-black/70">{item.value}</p>
            </div>
          ))}
        </div>
      </div>
    </article>
  );
}
