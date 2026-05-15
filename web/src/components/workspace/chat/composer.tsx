/**
 * Bottom message composer for the chat pane.
 *
 * Controls a textarea draft with `Enter` to submit, `Shift+Enter` for newline.
 * Includes a disabled context picker entrypoint and a Plan/Build mode toggle
 * until the backend exposes writable settings for them.
 */
"use client";

import { useState, type KeyboardEvent } from "react";

import { IconAt, IconSend } from "@/components/icons";
import { cn } from "@/lib/utils";

/** Composer mode discriminator. "Plan" is read-only, "Build" allows mutating tools. */
type Mode = "Plan" | "Build";

/** Props for {@link Composer}. */
type Props = {
  /** Submit handler; receives the trimmed draft and is responsible for delivering it. */
  onSubmit: (draft: string) => void;
  /** When true the composer renders in read-only mode — Phase 1 default. */
  disabled?: boolean;
  /** Optional explanation surfaced under the textarea when disabled. */
  disabledReason?: string;
};

/**
 * Renders the composer input + toolbar.
 *
 * Boundary: an all-whitespace draft is treated as empty — the Send button is
 * disabled and the keyboard shortcut is a no-op.
 */
export function Composer({ onSubmit, disabled = false, disabledReason }: Props) {
  const [draft, setDraft] = useState("");
  const [mode, setMode] = useState<Mode>("Plan");

  function submit() {
    if (disabled) return;
    if (!draft.trim()) return;
    onSubmit(draft);
    setDraft("");
  }

  function onKey(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  const canSend = !disabled && draft.trim().length > 0;

  return (
    <div className="border-t border-rule bg-paper px-8 pb-5 pt-3">
      <div className="rounded-[10px] border border-rule-2 bg-paper shadow-[0_1px_0_rgba(0,0,0,0.02),0_2px_8px_-2px_rgba(0,0,0,0.04)]">
        <div className="flex items-center gap-1.5 border-b border-rule px-2.5 py-2">
          <button
            type="button"
            disabled
            title="Context picker is not connected yet"
            className="inline-flex items-center gap-1.5 rounded-sm border border-rule bg-paper-2 px-2 py-1 text-[11.5px] text-ink-2"
          >
            <IconAt size={12} /> Add context
          </button>
          <div className="flex-1" />
          <ModeToggle mode={mode} onChange={setMode} />
        </div>
        <textarea
          rows={2}
          placeholder={
            disabled
              ? (disabledReason ?? "Browser write control is not yet attached")
              : "Reply to the agent, or steer the next step…"
          }
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKey}
          disabled={disabled}
          className={cn(
            "min-h-[44px] w-full resize-none border-none bg-transparent px-3.5 py-3 text-[13.5px] leading-[1.55] outline-none placeholder:text-ink-3",
            disabled ? "text-ink-3" : "text-ink",
          )}
        />
        <div className="flex items-center justify-between px-3.5 pb-2 pt-1.5">
          <div className="flex items-center gap-2.5 text-[11px] text-ink-3">
            {disabled ? (
              <span>{disabledReason ?? "read-only · browser write disabled"}</span>
            ) : (
              <span>read-only tools in Phase 0/1, sandboxed in Phase 2</span>
            )}
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
