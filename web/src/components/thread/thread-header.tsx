import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";

import { commandPreview, currentBranch } from "./thread-data";

export function ThreadHeader() {
  return (
    <header className="flex flex-col gap-3 border-b border-black/10 bg-[var(--change-main)] px-6 py-4 sm:px-10 md:h-16 md:flex-row md:items-center md:justify-between">
      <div className="flex flex-wrap items-center gap-4">
        <h1 className="font-mono text-xs uppercase tracking-[0.32em] text-black/80">
          Change List Control
        </h1>
        <Separator
          orientation="vertical"
          className="hidden h-4 bg-black/15 md:block"
        />
        <div className="flex items-center gap-2 font-mono text-xs text-black/60">
          <span>BRANCH:</span>
          <span className="text-black">{currentBranch}</span>
        </div>
      </div>

      <p className="font-mono text-[10px] tracking-[0.28em] text-[var(--change-green)] animate-pulse">
        [ LIVE STREAM ACTIVE ]
      </p>
    </header>
  );
}

export function ThreadToolbar() {
  return (
    <div className="flex flex-col gap-4 border-b border-black/10 bg-[var(--change-main)] px-6 py-5 sm:px-10 md:flex-row md:items-center md:gap-3">
      <span
        aria-hidden
        className="font-mono text-sm text-[var(--change-green)]"
      >
        $
      </span>

      <Input
        readOnly
        aria-label="Rendered change list command"
        value={commandPreview}
        className="h-auto border-none bg-transparent px-0 py-0 font-mono text-sm text-black/70 shadow-none focus-visible:ring-0"
      />

      <div className="flex flex-wrap items-center gap-4 md:justify-end">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-auto rounded-none px-0 py-0 font-mono text-[10px] tracking-[0.2em] text-black/60 hover:bg-transparent hover:text-black"
        >
          FILTER_VIEW
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-auto rounded-none px-0 py-0 font-mono text-[10px] tracking-[0.2em] text-black/60 hover:bg-transparent hover:text-black"
        >
          EXPORT_LOGS
        </Button>
      </div>
    </div>
  );
}
