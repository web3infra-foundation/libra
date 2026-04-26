"use client";

import { useEffect, useRef, useState } from "react";

import { IconBranch, IconCopy, IconMore, IconTerm, IconThread } from "@/components/icons";
import { Splitter } from "@/components/workspace/splitter";
import { Terminal } from "@/components/workspace/terminal/terminal";
import { MESSAGES, type ChatMessage } from "@/lib/mock";
import { useStoredBoolean, useStoredNumber } from "@/lib/persisted-state";
import { clamp } from "@/lib/storage";

import { Composer } from "./composer";
import { Message } from "./message";

export function Chat() {
  const [messages, setMessages] = useState<ChatMessage[]>(MESSAGES);
  const [termOpen, setTermOpen] = useStoredBoolean("libra.termOpen", true);
  const [termH, setTermH] = useStoredNumber("libra.termH", 240);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const chatBodyRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  // Stream the last assistant message that arrived flagged as streaming.
  useEffect(() => {
    const last = messages[messages.length - 1];
    if (!last || !last.streaming) return;
    const full = last.body;
    let i = 0;
    setMessages((m) => {
      const copy = [...m];
      copy[copy.length - 1] = { ...copy[copy.length - 1], body: "" };
      return copy;
    });
    const handle = window.setInterval(() => {
      i += 3;
      setMessages((m) => {
        const copy = [...m];
        const idx = copy.length - 1;
        copy[idx] = { ...copy[idx], body: full.slice(0, i) };
        return copy;
      });
      if (i >= full.length) {
        window.clearInterval(handle);
        setMessages((m) => {
          const copy = [...m];
          const idx = copy.length - 1;
          copy[idx] = { ...copy[idx], streaming: false };
          return copy;
        });
      }
    }, 26);
    return () => window.clearInterval(handle);
    // We only want this to run once, on the initial render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function submit(draft: string) {
    if (!draft.trim()) return;
    const time = nowTime();
    const userMsg: ChatMessage = {
      id: `u-${Date.now()}`,
      role: "user",
      time,
      body: draft,
    };
    const target = stubReply(draft);
    const assistantMsg: ChatMessage = {
      id: `a-${Date.now()}`,
      role: "assistant",
      time,
      body: "",
      streaming: true,
    };
    setMessages((m) => [...m, userMsg, assistantMsg]);

    let i = 0;
    const id = assistantMsg.id;
    window.setTimeout(() => {
      const handle = window.setInterval(() => {
        i += 3;
        setMessages((curr) => {
          const idx = curr.findIndex((m) => m.id === id);
          if (idx === -1) {
            window.clearInterval(handle);
            return curr;
          }
          const copy = [...curr];
          copy[idx] = { ...copy[idx], body: target.slice(0, i) };
          return copy;
        });
        if (i >= target.length) {
          window.clearInterval(handle);
          setMessages((curr) => {
            const idx = curr.findIndex((m) => m.id === id);
            if (idx === -1) return curr;
            const copy = [...curr];
            copy[idx] = { ...copy[idx], streaming: false };
            return copy;
          });
        }
      }, 22);
    }, 120);
  }

  function onTerminalDrag(dy: number, startH: number) {
    const parentH = chatBodyRef.current?.parentElement?.clientHeight ?? 800;
    const max = parentH - 260;
    setTermH(clamp(startH - dy, 120, max));
  }

  return (
    <section className="flex min-w-0 flex-1 flex-col bg-paper">
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-rule px-5">
        <div className="flex min-w-0 items-center gap-2.5">
          <IconThread size={15} />
          <div className="overflow-hidden text-ellipsis whitespace-nowrap text-[13.5px] font-semibold">
            Add optimistic updates to useMutation
          </div>
          <span className="mono inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-sm border border-rule-2 bg-paper-2 px-1.5 py-0.5 text-[10.5px] text-ink-2">
            <IconBranch size={11} /> agent/optimistic-mutate
          </span>
          <span className="mono inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-sm border border-accent-line bg-accent-soft px-1.5 py-0.5 text-[10.5px] text-accent">
            Phase 2 · Execution
          </span>
        </div>
        <div className="flex items-center gap-1 text-ink-3">
          {!termOpen && (
            <button
              type="button"
              onClick={() => setTermOpen(true)}
              className="mr-1 inline-flex items-center gap-1.5 rounded-sm border border-rule-2 bg-paper-2 px-2 py-1 text-[11px] text-ink-2"
              title="Show terminal"
            >
              <IconTerm size={13} /> Terminal
            </button>
          )}
          <button
            type="button"
            className="grid h-7 w-7 place-items-center rounded-md text-ink-3"
            title="Share"
          >
            <IconCopy size={14} />
          </button>
          <button
            type="button"
            className="grid h-7 w-7 place-items-center rounded-md text-ink-3"
            title="More"
          >
            <IconMore size={14} />
          </button>
        </div>
      </header>

      <div ref={chatBodyRef} className="flex min-h-0 flex-1 flex-col">
        <div ref={scrollRef} className="flex-1 overflow-y-auto px-8 pb-5 pt-6">
          <div className="mb-[22px] flex items-center gap-2.5">
            <div className="h-px flex-1 bg-rule" />
            <div className="mono text-[11px] text-ink-3">
              thread opened 10:42 · intent confirmed · 2 plan revisions
            </div>
            <div className="h-px flex-1 bg-rule" />
          </div>
          {messages.map((m) => (
            <Message key={m.id} message={m} />
          ))}
        </div>
        <Composer onSubmit={submit} />
      </div>

      {termOpen && (
        <>
          <Splitter
            orientation="horizontal"
            value={termH}
            onDrag={onTerminalDrag}
          />
          <Terminal height={termH} onClose={() => setTermOpen(false)} />
        </>
      )}
    </section>
  );
}

function nowTime() {
  const d = new Date();
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function stubReply(input: string) {
  const short = input.trim().split(/\s+/).slice(0, 6).join(" ");
  return `Got it — "${short}…". I'll re-read the relevant files and draft a revised execution plan; the test plan stays unless I need new coverage. One moment.`;
}
