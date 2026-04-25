"use client";

import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";

import { IconSpark, IconTerm, IconTool, IconX } from "@/components/icons";
import { TERMINAL_LINES, type TerminalLine, type TerminalLineKind } from "@/lib/mock";
import { cn } from "@/lib/utils";

type Tab = "sandbox" | "tools" | "agent";

type Props = {
  height: number;
  onClose: () => void;
};

export function Terminal({ height, onClose }: Props) {
  const [tab, setTab] = useState<Tab>("sandbox");
  const [cmd, setCmd] = useState("");
  const [history, setHistory] = useState<TerminalLine[]>(TERMINAL_LINES);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [history]);

  function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (!cmd.trim()) return;
    const entered: TerminalLine = { kind: "prompt", text: cmd };
    const reply: TerminalLine = { kind: "stdout", text: stubShellReply(cmd) };
    setHistory((h) => [...h, entered, reply]);
    setCmd("");
  }

  const visible = history.filter((l) => filterByTab(l, tab));

  return (
    <div
      className="flex shrink-0 flex-col overflow-hidden border-t border-rule-2 bg-paper-2"
      style={{ height, minHeight: 120 }}
    >
      <header className="flex h-[34px] shrink-0 items-center justify-between border-b border-rule bg-paper px-4 py-0">
        <div className="flex gap-px">
          <TermTab active={tab === "sandbox"} onClick={() => setTab("sandbox")}>
            <IconTerm size={12} /> Sandbox
          </TermTab>
          <TermTab active={tab === "tools"} onClick={() => setTab("tools")}>
            <IconTool size={12} /> Tools
          </TermTab>
          <TermTab active={tab === "agent"} onClick={() => setTab("agent")}>
            <IconSpark size={12} /> Agent
          </TermTab>
        </div>
        <div className="flex items-center gap-1.5 text-[11px] text-ink-3">
          <span
            className="h-[7px] w-[7px] rounded-full bg-good"
            style={{ boxShadow: "0 0 0 2px color-mix(in oklch, var(--good) 22%, transparent)" }}
          />
          <span className="mono text-[10.5px]">libra-sbx-04</span>
          <span className="text-rule-2">·</span>
          <span className="mono text-[10.5px]">rust:1.81 · net=off</span>
          <button
            type="button"
            onClick={onClose}
            className="ml-1 grid h-[22px] w-[22px] place-items-center rounded-sm text-ink-3"
            title="Hide terminal"
          >
            <IconX size={12} />
          </button>
        </div>
      </header>

      <div ref={scrollRef} className="flex-1 overflow-y-auto bg-paper-2 px-4 py-2">
        {visible.map((l, i) => (
          <TermLineRow key={i} line={l} />
        ))}
      </div>

      <form
        onSubmit={submit}
        className="flex shrink-0 items-center gap-2 border-t border-rule bg-paper px-4 py-2"
      >
        <span className="mono shrink-0 text-[11px] font-medium text-accent">
          agent@sbx-04 ~ $
        </span>
        <input
          value={cmd}
          onChange={(e) => setCmd(e.target.value)}
          placeholder="run a command in the sandbox…"
          spellCheck={false}
          className="mono flex-1 border-none bg-transparent p-0 text-[11.5px] text-ink outline-none placeholder:text-ink-3"
        />
      </form>
    </div>
  );
}

function TermTab({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "-mb-px inline-flex items-center gap-1.5 border-b-[1.5px] px-2.5 py-1 text-[11.5px] font-medium",
        active ? "border-ink text-ink" : "border-transparent text-ink-3",
      )}
    >
      {children}
    </button>
  );
}

function TermLineRow({ line }: { line: TerminalLine }) {
  const tone = lineTone(line.kind);
  const showMark = line.kind === "prompt";
  return (
    <div className="flex items-baseline gap-2 py-[1.5px]">
      <span
        className="mono w-3.5 shrink-0 text-[10.5px]"
        style={{ color: tone.mark }}
      >
        {lineMark(line.kind)}
      </span>
      <span
        className={cn(
          "mono flex-1 whitespace-pre-wrap break-words text-[11.5px] leading-[1.55]",
          showMark ? "font-medium" : "font-normal",
        )}
        style={{ color: tone.text }}
      >
        {line.text || " "}
      </span>
    </div>
  );
}

function lineMark(kind: TerminalLineKind) {
  switch (kind) {
    case "prompt":
      return "$";
    case "pass":
      return "✓";
    case "fail":
      return "✗";
    case "run":
      return "•";
    case "warn":
      return "!";
    case "info":
      return "ℹ";
    case "meta":
      return "·";
    default:
      return " ";
  }
}

function lineTone(kind: TerminalLineKind) {
  switch (kind) {
    case "prompt":
      return { mark: "var(--accent)", text: "var(--ink)" };
    case "pass":
      return { mark: "var(--good)", text: "var(--ink-2)" };
    case "fail":
      return { mark: "var(--bad)", text: "var(--bad)" };
    case "run":
      return { mark: "var(--accent)", text: "var(--ink-2)" };
    case "warn":
      return { mark: "var(--warn)", text: "var(--ink-2)" };
    case "info":
      return { mark: "var(--accent)", text: "var(--ink-2)" };
    case "meta":
      return { mark: "var(--ink-3)", text: "var(--ink-3)" };
    default:
      return { mark: "var(--ink-3)", text: "var(--ink-2)" };
  }
}

function filterByTab(line: TerminalLine, tab: Tab) {
  if (tab === "sandbox") return true;
  if (tab === "tools")
    return line.kind === "prompt" || line.kind === "stdout" || line.kind === "meta";
  if (tab === "agent")
    return line.kind === "info" || line.kind === "warn" || line.kind === "meta";
  return true;
}

function stubShellReply(cmd: string) {
  const c = cmd.trim().toLowerCase();
  if (c === "ls") return "Cargo.toml  src/  tests/  target/";
  if (c.startsWith("cat "))
    return `# ${c.slice(4)}\n(sandboxed — preview truncated)`;
  if (c.startsWith("cargo"))
    return "error: sandbox locked to agent execution. Use the agent to re-run tests.";
  return `command not found: ${cmd.split(/\s+/)[0] ?? cmd}`;
}
