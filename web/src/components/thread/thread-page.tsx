import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";

import { compactChanges, featuredChange } from "./thread-data";
import { ThreadHeader, ThreadToolbar } from "./thread-header";
import { CompactChangeCard, FeaturedChangeCard } from "./thread-item-card";
import { ThreadLayout } from "./thread-layout";
import styles from "./thread-page.module.css";
import { ThreadSidebar } from "./thread-sidebar";

export function ThreadPage() {
  return (
    <div
      className={cn(
        "min-h-screen [--change-shell:#0a0a0a] [--change-sidebar:#f8f8f8] [--change-main:#f5f5f5] [--change-green:#16a34a] [--change-blue:#3b82f6] [--change-purple:#a855f7]",
      )}
    >
      <ThreadLayout>
        <ThreadLayout.Sidebar>
          <ThreadSidebar />
        </ThreadLayout.Sidebar>

        <ThreadLayout.Main>
          <ThreadHeader />
          <ThreadToolbar />

          <section
            className={cn(
              "flex-1 overflow-y-auto px-6 py-10 pb-32 sm:px-10 sm:py-12",
              styles.scrollRegion,
            )}
            aria-label="Monorepo change list"
          >
            <div className="space-y-16">
              <FeaturedChangeCard change={featuredChange} />

              {compactChanges.map((change) => (
                <div key={change.id} className="space-y-16">
                  <Separator className="bg-black/10" />
                  <CompactChangeCard change={change} />
                </div>
              ))}
            </div>
          </section>

          <div
            aria-hidden
            className="pointer-events-none absolute inset-x-0 bottom-0 h-24 bg-gradient-to-t from-white via-white/85 to-transparent"
          />
        </ThreadLayout.Main>
      </ThreadLayout>
    </div>
  );
}
