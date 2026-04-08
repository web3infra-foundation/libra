import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

interface LayoutSectionProps {
  children: ReactNode;
  className?: string;
}

function ThreadLayoutRoot({ children, className }: LayoutSectionProps) {
  return (
    <div
      className={cn(
        "flex min-h-screen flex-col bg-[var(--change-shell)] text-white lg:h-screen lg:flex-row lg:overflow-hidden",
        className,
      )}
    >
      {children}
    </div>
  );
}

function ThreadLayoutSidebar({ children, className }: LayoutSectionProps) {
  return (
    <aside
      className={cn(
        "z-20 w-full shrink-0 border-b border-black/10 bg-[var(--change-sidebar)] text-black lg:h-full lg:w-72 lg:border-r lg:border-b-0",
        className,
      )}
    >
      {children}
    </aside>
  );
}

function ThreadLayoutMain({ children, className }: LayoutSectionProps) {
  return (
    <main
      className={cn(
        "relative flex min-h-[32rem] flex-1 flex-col bg-[var(--change-main)] text-black lg:h-full",
        className,
      )}
    >
      {children}
    </main>
  );
}

export const ThreadLayout = Object.assign(ThreadLayoutRoot, {
  Sidebar: ThreadLayoutSidebar,
  Main: ThreadLayoutMain,
});
