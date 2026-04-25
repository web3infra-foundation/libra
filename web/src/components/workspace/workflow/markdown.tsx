"use client";

import { Fragment, useMemo, type JSX, type ReactNode } from "react";

type Block =
  | { type: "h"; level: 1 | 2 | 3; text: string }
  | { type: "p"; text: string }
  | { type: "ul"; items: string[] }
  | { type: "ol"; items: string[] };

type Props = {
  source: string;
};

export function Markdown({ source }: Props) {
  const blocks = useMemo(() => parse(source), [source]);
  return (
    <div className="text-[13px] leading-[1.65] text-ink">
      {blocks.map((b, i) => (
        <Fragment key={i}>{renderBlock(b)}</Fragment>
      ))}
    </div>
  );
}

function parse(src: string): Block[] {
  const lines = src.split("\n");
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }

    const h = /^(#{1,3})\s+(.*)$/.exec(line);
    if (h) {
      out.push({ type: "h", level: h[1].length as 1 | 2 | 3, text: h[2] });
      i++;
      continue;
    }

    if (/^\s*-\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*-\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*-\s+/, ""));
        i++;
      }
      out.push({ type: "ul", items });
      continue;
    }

    if (/^\s*\d+\.\s+/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*\d+\.\s+/, ""));
        i++;
      }
      out.push({ type: "ol", items });
      continue;
    }

    const para = [line];
    i++;
    while (
      i < lines.length &&
      lines[i].trim() &&
      !/^(#{1,3}\s|\s*-\s|\s*\d+\.\s)/.test(lines[i])
    ) {
      para.push(lines[i]);
      i++;
    }
    out.push({ type: "p", text: para.join(" ") });
  }
  return out;
}

function renderBlock(b: Block) {
  if (b.type === "h") {
    const Tag = (`h${b.level}`) as keyof JSX.IntrinsicElements;
    const cls =
      b.level === 1
        ? "text-[19px] font-semibold leading-[1.25] tracking-[-0.015em] mb-2.5 mt-0"
        : b.level === 2
          ? "text-[13px] font-semibold leading-[1.3] tracking-[-0.005em] mt-[22px] mb-2 pb-1 border-b border-rule"
          : "text-[12.5px] font-semibold leading-[1.3] mt-[18px] mb-1.5 text-ink-2";
    return <Tag className={cls}>{renderInline(b.text)}</Tag>;
  }
  if (b.type === "p") {
    return (
      <p className="mb-2.5 text-[13px] leading-[1.65] text-ink-2">
        {renderInline(b.text)}
      </p>
    );
  }
  if (b.type === "ul") {
    return (
      <ul className="mb-3 list-disc pl-[18px]">
        {b.items.map((it, i) => (
          <li key={i} className="my-[3px] text-[13px] leading-[1.6] text-ink-2">
            {renderInline(it)}
          </li>
        ))}
      </ul>
    );
  }
  return (
    <ol className="mb-3 list-decimal pl-[20px]">
      {b.items.map((it, i) => (
        <li key={i} className="my-[3px] text-[13px] leading-[1.6] text-ink-2">
          {renderInline(it)}
        </li>
      ))}
    </ol>
  );
}

function renderInline(text: string): ReactNode[] {
  const parts: ReactNode[] = [];
  const regex = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*]+\*)/g;
  let last = 0;
  let k = 0;
  let m: RegExpExecArray | null;
  while ((m = regex.exec(text)) !== null) {
    if (m.index > last) parts.push(text.slice(last, m.index));
    const tok = m[0];
    if (tok.startsWith("`")) {
      parts.push(
        <code
          key={k++}
          className="mono rounded-sm border border-rule bg-paper-2 px-1.5 py-px text-[11.5px] text-ink"
        >
          {tok.slice(1, -1)}
        </code>,
      );
    } else if (tok.startsWith("**")) {
      parts.push(
        <strong key={k++} className="font-semibold text-ink">
          {tok.slice(2, -2)}
        </strong>,
      );
    } else {
      parts.push(<em key={k++}>{tok.slice(1, -1)}</em>);
    }
    last = m.index + tok.length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}
