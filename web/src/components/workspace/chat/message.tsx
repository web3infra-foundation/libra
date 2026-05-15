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
import type { CodeUiTranscriptEntryKind } from "@/lib/code-ui/types";
import { cn } from "@/lib/utils";

/** Chat-pane message shape — derived from snapshot transcript entries. */
export type ChatMessageView = {
  id: string;
  role: "user" | "assistant";
  time: string;
  body: string;
  fullBody?: string;
  hiddenChars?: number;
  streaming?: boolean;
  /** Optional decorator hints from the upstream transcript entry. */
  kind?: CodeUiTranscriptEntryKind;
  title?: string;
};

/** Props shared by both message variants. */
type Props = {
  message: ChatMessageView;
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
  const [expanded, setExpanded] = useState(false);
  const displayedBody = expanded && message.fullBody ? message.fullBody : message.body;

  function copy() {
    const text = message.fullBody ?? message.body;
    const done = () => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    };
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(text).then(done, () => fallbackCopy(text, done));
    } else {
      fallbackCopy(text, done);
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
          {displayedBody}
        </div>
      </div>
      {message.fullBody && (
        <ExpandMessageButton
          expanded={expanded}
          hiddenChars={message.hiddenChars}
          onClick={() => setExpanded((value) => !value)}
        />
      )}
      <div className="mt-1 flex gap-1.5 text-[10.5px] text-ink-3">
        <span className="mono">you</span>
        <span>·</span>
        <span>{message.time}</span>
      </div>
    </div>
  );
}

function AssistantMessage({ message }: Props) {
  const [expanded, setExpanded] = useState(false);
  const displayedBody = expanded && message.fullBody ? message.fullBody : message.body;

  return (
    <div className="mb-[26px] max-w-[720px]">
      <div className="mb-2 flex items-center gap-1.5 text-ink-2">
        <div className="grid h-[18px] w-[18px] place-items-center">
          <BrandMark size={18} />
        </div>
        <span className="mono text-[10.5px] font-medium">libra</span>
        <span className="text-[10.5px] text-ink-3">·</span>
        <span className="text-[10.5px] text-ink-3">{message.time}</span>
        {message.title && (
          <span className="text-[10.5px] text-ink-3">· {message.title}</span>
        )}
        {message.streaming && (
          <span className="mono ml-2 inline-flex items-center gap-1.5 rounded-sm bg-accent-soft px-1.5 py-px text-[10px] text-accent">
            <span className="libra-pulse h-[5px] w-[5px] rounded-full bg-accent" /> streaming
          </span>
        )}
      </div>
      <div className="whitespace-pre-wrap border-l border-rule pl-6 text-[13.5px] leading-[1.62] text-ink">
        {displayedBody.split("\n").map((line, i) => (
          <div key={i} style={{ minHeight: "1em" }}>
            {line}
          </div>
        ))}
        {message.streaming && <span className="libra-caret" />}
        {message.fullBody && (
          <ExpandMessageButton
            expanded={expanded}
            hiddenChars={message.hiddenChars}
            onClick={() => setExpanded((value) => !value)}
          />
        )}
      </div>
    </div>
  );
}

function ExpandMessageButton({
  expanded,
  hiddenChars,
  onClick,
}: {
  expanded: boolean;
  hiddenChars?: number;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="mt-2 rounded-sm border border-rule bg-paper px-1.5 py-0.5 text-[10.5px] font-medium text-ink-3 hover:text-ink"
    >
      {expanded ? "Show less" : `Show full message (${hiddenChars?.toLocaleString()} chars hidden)`}
    </button>
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
