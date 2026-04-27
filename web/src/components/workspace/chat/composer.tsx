/**
 * Bottom message composer for the chat pane.
 *
 * Controls a textarea draft with `Enter` to submit, `Shift+Enter` for newline.
 * Includes static "Add context" / file chips (placeholders for future
 * features) and a Plan/Build mode toggle. The actual model name and tool
 * policy strings are static — the production backend will surface these
 * through a session settings RPC.
 */
"use client";

import { useState, type KeyboardEvent } from "react";

import { IconAt, IconFile, IconSend } from "@/components/icons";
import { cn } from "@/lib/utils";

/** Composer mode discriminator. "Plan" is read-only, "Build" allows mutating tools. */
type Mode = "Plan" | "Build";

/** Props for {@link Composer}. */
type Props = {
  /** Submit handler; receives the trimmed draft and is responsible for delivering it. */
  onSubmit: (draft: string) => void;
};

/**
 * Renders the composer input + toolbar.
 *
 * Boundary: an all-whitespace draft is treated as empty — the Send button is
 * disabled and the keyboard shortcut is a no-op.
 */
export function Composer({ onSubmit }: Props) {
  const [draft, setDraft] = useState("");
  const [mode, setMode] = useState<Mode>("Plan");

  function submit() {
    if (!draft.trim()) return;
    onSubmit(draft);
    setDraft("");
  }

  // Enter submits, Shift+Enter inserts a newline (the textarea native default
  // for Shift+Enter is preserved by not preventing default in that branch).
  function onKey(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  const canSend = draft.trim().length > 0;

  return (
    <div className="border-t border-rule bg-paper px-8 pb-5 pt-3">
      <div className="rounded-[10px] border border-rule-2 bg-paper shadow-[0_1px_0_rgba(0,0,0,0.02),0_2px_8px_-2px_rgba(0,0,0,0.04)]">
        <div className="flex items-center gap-1.5 border-b border-rule px-2.5 py-2">
          <button
            type="button"
            className="inline-flex items-center gap-1.5 rounded-sm border border-rule bg-paper-2 px-2 py-1 text-[11.5px] text-ink-2"
          >
            <IconAt size={12} /> Add context
          </button>
          <button
            type="button"
            className="inline-flex items-center gap-1.5 rounded-sm border border-rule bg-paper-2 px-2 py-1 text-[11.5px] text-ink-2"
          >
            <IconFile size={12} /> src/lib/query.ts
          </button>
          <div className="flex-1" />
          <ModeToggle mode={mode} onChange={setMode} />
        </div>
        <textarea
          rows={2}
          placeholder="Reply to the agent, or steer the next step…"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKey}
          className="min-h-[44px] w-full resize-none border-none bg-transparent px-3.5 py-3 text-[13.5px] leading-[1.55] text-ink outline-none placeholder:text-ink-3"
        />
        <div className="flex items-center justify-between px-3.5 pb-2 pt-1.5">
          <div className="flex items-center gap-2.5 text-[11px] text-ink-3">
            <span className="mono text-[10.5px]">claude-sonnet-4.5</span>
            <span>·</span>
            <span>read-only tools in Phase 0/1, sandboxed in Phase 2</span>
          </div>
          <button
            type="button"
            onClick={submit}
            disabled={!canSend}
            className={cn(
              "inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-[12px] font-medium transition-colors",
              canSend
                ? "border-accent bg-accent text-white"
                : "border-rule bg-paper-2 text-ink-3",
            )}
          >
            <IconSend size={13} /> Send
            <span className="mono ml-0.5 text-[10px] opacity-80">↵</span>
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * Two-state segmented control for the composer mode.
 *
 * The selected mode renders with a paper background + tiny shadow; the other
 * mode is muted text only.
 */
function ModeToggle({ mode, onChange }: { mode: Mode; onChange: (m: Mode) => void }) {
  const options: Mode[] = ["Plan", "Build"];
  return (
    <div className="flex gap-0.5 rounded-md border border-rule bg-paper-2 p-0.5">
      {options.map((m) => (
        <button
          key={m}
          type="button"
          onClick={() => onChange(m)}
          className={cn(
            "rounded-sm px-2.5 py-1 text-[11.5px] font-medium",
            mode === m
              ? "bg-paper text-ink shadow-[0_1px_0_rgba(0,0,0,0.04)]"
              : "text-ink-3 hover:text-ink-2",
          )}
        >
          {m}
        </button>
      ))}
    </div>
  );
}
