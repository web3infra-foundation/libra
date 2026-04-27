/**
 * Chat message renderers.
 *
 * Two visual variants:
 *  - **User**: right-aligned dark bubble with hover-revealed Copy button.
 *  - **Assistant**: left-aligned, full width, with the Libra brand mark and
 *    optional streaming caret/badge while text is being typed in.
 *
 * The Copy button uses the async clipboard API when available and falls
 * back to the legacy `document.execCommand("copy")` flow for older browsers
 * and sandboxed contexts where the secure API is unavailable.
 */
"use client";

import { useState } from "react";

import { IconCheck, IconCopy } from "@/components/icons";
import { BrandMark } from "@/components/workspace/brand-mark";
import type { ChatMessage } from "@/lib/mock";
import { cn } from "@/lib/utils";

/** Props shared by both message variants. */
type Props = {
  message: ChatMessage;
};

/**
 * Top-level message dispatcher — renders user or assistant variant based on
 * the message role.
 */
export function Message({ message }: Props) {
  if (message.role === "user") {
    return <UserMessage message={message} />;
  }
  return <AssistantMessage message={message} />;
}

/**
 * User message bubble.
 *
 * Hover reveals a Copy affordance. After a successful copy the button shows
 * a "Copied" confirmation for ~1.4 s before reverting.
 */
function UserMessage({ message }: Props) {
  const [hover, setHover] = useState(false);
  const [copied, setCopied] = useState(false);

  function copy() {
    const done = () => {
      setCopied(true);
      // Auto-revert the "Copied" badge after a beat so the affordance reads
      // as confirmation rather than a permanent state change.
      setTimeout(() => setCopied(false), 1400);
    };
    // Prefer the async clipboard API; if it rejects (permissions, insecure
    // context) or is unavailable, fall back to the textarea + execCommand
    // approach which works in more environments.
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(message.body).then(done, () => fallbackCopy(message.body, done));
    } else {
      fallbackCopy(message.body, done);
    }
  }

  return (
    <div
      className="mb-[22px] flex flex-col items-end"
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <div className="flex max-w-[82%] items-end gap-2">
        <button
          type="button"
          onClick={copy}
          title={copied ? "Copied" : "Copy message"}
          className={cn(
            "mb-0.5 inline-flex shrink-0 items-center gap-1 rounded-sm border bg-paper px-1.5 py-0.5 text-[10.5px] font-medium transition-[opacity,color,border-color] duration-150",
            copied ? "border-good text-good" : "border-rule-2 text-ink-3",
            hover || copied ? "pointer-events-auto opacity-100" : "pointer-events-none opacity-0",
          )}
        >
          {copied ? <IconCheck size={12} sw={2.5} /> : <IconCopy size={12} />}
          <span className="text-[10.5px]">{copied ? "Copied" : "Copy"}</span>
        </button>
        <div className="max-w-[78%] whitespace-pre-wrap rounded-[10px_10px_2px_10px] bg-ink px-3.5 py-2.5 text-[13px] leading-[1.55] text-paper">
          {message.body}
        </div>
      </div>
      <div className="mt-1 flex gap-1.5 text-[10.5px] text-ink-3">
        <span className="mono">you</span>
        <span>·</span>
        <span>{message.time}</span>
      </div>
    </div>
  );
}

function AssistantMessage({ message }: Props) {
  return (
    <div className="mb-[26px] max-w-[720px]">
      <div className="mb-2 flex items-center gap-1.5 text-ink-2">
        <div className="grid h-[18px] w-[18px] place-items-center">
          <BrandMark size={18} />
        </div>
        <span className="mono text-[10.5px] font-medium">libra</span>
        <span className="text-[10.5px] text-ink-3">·</span>
        <span className="text-[10.5px] text-ink-3">{message.time}</span>
        {message.streaming && (
          <span className="mono ml-2 inline-flex items-center gap-1.5 rounded-sm bg-accent-soft px-1.5 py-px text-[10px] text-accent">
            <span className="libra-pulse h-[5px] w-[5px] rounded-full bg-accent" /> streaming
          </span>
        )}
      </div>
      <div className="whitespace-pre-wrap border-l border-rule pl-6 text-[13.5px] leading-[1.62] text-ink">
        {message.body.split("\n").map((line, i) => (
          <div key={i} style={{ minHeight: "1em" }}>
            {line}
          </div>
        ))}
        {message.streaming && <span className="libra-caret" />}
      </div>
    </div>
  );
}

function fallbackCopy(text: string, done: () => void) {
  if (typeof document === "undefined") return;
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    document.body.removeChild(ta);
    done();
  } catch {
    // ignore
  }
}
