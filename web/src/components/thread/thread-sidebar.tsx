import { cn } from "@/lib/utils";

import {
  branchTopology,
  collaborators,
  repositorySource,
} from "./thread-data";
import styles from "./thread-page.module.css";

const topologyTextClassNames = {
  muted: "text-black/45",
  active: "font-semibold text-black",
  soft: "text-black/45",
} as const;

export function ThreadSidebar() {
  return (
    <div className="flex h-full flex-col">
      <section className="px-8 pb-6 pt-8">
        <p className="mb-2 font-mono text-[10px] uppercase tracking-[0.22em] text-black/45">
          Repository Source
        </p>
        <p className="text-sm font-bold tracking-[-0.03em]">
          {repositorySource.name}
        </p>
        <p className="mt-1 font-mono text-[10px] text-black/35">
          ID: {repositorySource.id}
        </p>
      </section>

      <section
        className={cn(
          "flex-1 px-8 py-4 lg:overflow-y-auto",
          styles.scrollRegion,
        )}
      >
        <h2 className="mb-6 font-mono text-[10px] uppercase tracking-[0.22em] text-black/45">
          Branch Topology
        </h2>

        <div className="relative h-64 pl-2 font-mono text-[10px]">
          <div aria-hidden className={styles.topologyLine} />

          {branchTopology.map((branch) => (
            <div key={branch.name} className="relative mb-8 pl-6">
              {branch.hasConnector ? (
                <div
                  aria-hidden
                  className="absolute left-2 top-2 h-px w-4 bg-black/15"
                />
              ) : null}
              {branch.hasSplit ? (
                <div
                  aria-hidden
                  className="absolute bottom-2 left-2 h-4 w-px bg-black/15"
                />
              ) : null}

              <div
                aria-hidden
                className={cn(
                  styles.topologyNode,
                  branch.tone === "active" && styles.topologyNodeActive,
                  branch.tone === "muted" && styles.topologyNodeMuted,
                  branch.tone === "soft" && styles.topologyNodeSoft,
                )}
              />

              <span className={topologyTextClassNames[branch.tone]}>
                {branch.name}
              </span>

              {branch.headLabel ? (
                <p className="mt-0.5 text-[9px] text-black/35">
                  {branch.headLabel}
                </p>
              ) : null}
            </div>
          ))}
        </div>
      </section>

      <section className="border-t border-black/8 px-8 py-6">
        <h2 className="mb-4 font-mono text-[10px] uppercase tracking-[0.22em] text-black/45">
          Collaborators (AI)
        </h2>
        <div className="flex flex-col gap-3 font-mono text-[11px]">
          {collaborators.map((collaborator) => (
            <button
              key={collaborator.code}
              type="button"
              className={cn(
                "group flex items-center justify-between text-left",
                collaborator.idle && "opacity-50",
              )}
            >
              <span className="text-black/80">{collaborator.code}</span>
              <span className="text-black/35 transition-colors group-hover:text-black">
                {collaborator.specialty}
              </span>
            </button>
          ))}
        </div>
      </section>

      <section className="mt-auto border-t border-black/8 p-8">
        <h2 className="mb-3 font-mono text-[10px] uppercase tracking-[0.22em] text-black/45">
          System Status
        </h2>
        <div
          className="flex items-center gap-2 font-mono text-[10px] text-black/90"
          role="status"
        >
          <span
            aria-hidden
            className="size-2 animate-pulse bg-black"
          />
          <span>PROCESSING QUEUE</span>
        </div>
      </section>
    </div>
  );
}
