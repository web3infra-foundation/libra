/**
 * View-model derivations from {@link CodeUiSessionSnapshot}.
 *
 * The store gives components the raw snapshot; these helpers shape it for the
 * chat pane, workflow strip, terminal log, and summary tab. Keeping them
 * pure (no React, no fetching) makes them trivial to unit-test.
 */

import type {
  CodeUiPatchsetSnapshot,
  CodeUiSessionSnapshot,
  CodeUiToolCallSnapshot,
  CodeUiTranscriptEntry,
} from "./types";

/** Two-letter clock string used in chat bubbles ("HH:MM"). */
function formatTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "--:--";
  const hh = String(date.getHours()).padStart(2, "0");
  const mm = String(date.getMinutes()).padStart(2, "0");
  return `${hh}:${mm}`;
}

export type ChatRole = "user" | "assistant" | "tool" | "info";

export type DerivedChatMessage = {
  id: string;
  role: ChatRole;
  time: string;
  body: string;
  fullBody?: string;
  hiddenChars?: number;
  streaming: boolean;
  /** Original transcript entry kind so callers can decorate non-text entries. */
  kind: CodeUiTranscriptEntry["kind"];
  /** Optional title/lead for `tool_call` / `plan_summary` / `info_note` entries. */
  title?: string;
};

export const CHAT_TRANSCRIPT_MESSAGE_LIMIT = 200;
export const CHAT_MESSAGE_PREVIEW_CHARS = 32 * 1024;

/**
 * Map snapshot transcript entries to the chat pane message list. All six
 * entry kinds round-trip:
 *   - `user_message` → `user` bubble
 *   - `assistant_message` → `assistant` bubble (respects `streaming`)
 *   - `tool_call` → `tool` row (details collapsed under the title)
 *   - `plan_summary` → `info` row with a "plan summary" prefix
 *   - `diff` → `info` row pointing at the patchset (full diff lives in Review)
 *   - `info_note` → `info` row, plain
 */
export function deriveChatMessages(
  snapshot: CodeUiSessionSnapshot | null,
): DerivedChatMessage[] {
  if (!snapshot) return [];
  const hiddenEntries = Math.max(0, snapshot.transcript.length - CHAT_TRANSCRIPT_MESSAGE_LIMIT);
  const transcript = hiddenEntries > 0
    ? snapshot.transcript.slice(hiddenEntries)
    : snapshot.transcript;
  const messages: DerivedChatMessage[] = transcript.map((entry): DerivedChatMessage => {
    const time = formatTime(entry.updatedAt);
    const body = truncateChatBody(entry.content ?? "");
    switch (entry.kind) {
      case "user_message":
        return { id: entry.id, role: "user", time, ...body, streaming: false, kind: entry.kind };
      case "assistant_message":
        return {
          id: entry.id,
          role: "assistant",
          time,
          ...body,
          streaming: entry.streaming,
          kind: entry.kind,
          title: entry.title,
        };
      case "tool_call":
        return {
          id: entry.id,
          role: "tool",
          time,
          ...body,
          streaming: false,
          kind: entry.kind,
          title: entry.title ?? "tool call",
        };
      case "plan_summary":
        return {
          id: entry.id,
          role: "info",
          time,
          ...body,
          streaming: false,
          kind: entry.kind,
          title: entry.title ?? "plan summary",
        };
      case "diff":
        return {
          id: entry.id,
          role: "info",
          time,
          ...body,
          streaming: false,
          kind: entry.kind,
          title: entry.title ?? "diff",
        };
      case "info_note":
        return {
          id: entry.id,
          role: "info",
          time,
          ...body,
          streaming: false,
          kind: entry.kind,
          title: entry.title,
        };
    }
  });
  if (hiddenEntries > 0) {
    messages.unshift({
      id: `transcript-collapsed-${hiddenEntries}`,
      role: "info",
      time: transcript[0] ? formatTime(transcript[0].updatedAt) : "--:--",
      body: `Transcript collapsed: ${hiddenEntries} earlier entries hidden.`,
      streaming: false,
      kind: "info_note",
      title: "transcript",
    });
  }
  return messages;
}

function truncateChatBody(body: string): {
  body: string;
  fullBody?: string;
  hiddenChars?: number;
} {
  if (body.length <= CHAT_MESSAGE_PREVIEW_CHARS) {
    return { body };
  }
  return {
    body: body.slice(0, CHAT_MESSAGE_PREVIEW_CHARS),
    fullBody: body,
    hiddenChars: body.length - CHAT_MESSAGE_PREVIEW_CHARS,
  };
}

export type WorkflowSummary = {
  planCount: number;
  taskCount: number;
  toolCallCount: number;
  patchsetCount: number;
  /** Pending interaction count — drives the InteractionPanel badge. */
  pendingInteractions: number;
};

export function deriveWorkflowSummary(
  snapshot: CodeUiSessionSnapshot | null,
): WorkflowSummary {
  if (!snapshot) {
    return {
      planCount: 0,
      taskCount: 0,
      toolCallCount: 0,
      patchsetCount: 0,
      pendingInteractions: 0,
    };
  }
  return {
    planCount: snapshot.plans.length,
    taskCount: snapshot.tasks.length,
    toolCallCount: snapshot.toolCalls.length,
    patchsetCount: snapshot.patchsets.length,
    pendingInteractions: snapshot.interactions.filter(
      (interaction) => interaction.status === "pending",
    ).length,
  };
}

export type TerminalRowKind = "meta" | "info" | "run" | "pass" | "fail" | "warn";
export const TERMINAL_OUTPUT_PREVIEW_CHARS = 200 * 1024;

export type TerminalRow = {
  kind: TerminalRowKind;
  text: string;
  fullText?: string;
  hiddenChars?: number;
};

/**
 * Derive a read-only terminal log from the snapshot. v1 stitches together:
 *   - sandbox / runtime banner from `provider`/`controller`
 *   - the latest tool calls (run / pass / fail rows)
 *   - `info_note` and `tool_call` transcript entries as `info` rows
 *
 * No actual shell execution happens in the browser — see Phase 2 for
 * agent-driven approval flows.
 */
export function deriveTerminalRows(
  snapshot: CodeUiSessionSnapshot | null,
): TerminalRow[] {
  if (!snapshot) return [];
  const rows: TerminalRow[] = [];
  rows.push({
    kind: "meta",
    text: `${snapshot.provider.provider}${
      snapshot.provider.model ? ` · ${snapshot.provider.model}` : ""
    } · ${snapshot.status}`,
  });

  for (const tool of snapshot.toolCalls) {
    rows.push(toolCallRow(tool));
  }

  for (const entry of snapshot.transcript) {
    if (entry.kind === "info_note" && entry.content) {
      rows.push({ kind: "info", text: entry.content });
    }
  }
  return rows;
}

function toolCallRow(tool: CodeUiToolCallSnapshot): TerminalRow {
  const kind = toolCallRowKind(tool.status);
  const heading = tool.summary ?? `${tool.toolName} (${tool.status})`;
  const text = tool.details ? `${heading}\n${tool.details}` : heading;
  return truncateTerminalRow(kind, text);
}

function toolCallRowKind(status: string): TerminalRowKind {
  const lower = status.toLowerCase();
  if (lower === "succeeded" || lower === "completed") return "pass";
  if (lower === "failed" || lower === "error" || lower === "cancelled") return "fail";
  if (lower === "running" || lower === "preview") return "run";
  return "info";
}

function truncateTerminalRow(kind: TerminalRowKind, text: string): TerminalRow {
  if (text.length <= TERMINAL_OUTPUT_PREVIEW_CHARS) {
    return { kind, text };
  }
  return {
    kind,
    text: text.slice(0, TERMINAL_OUTPUT_PREVIEW_CHARS),
    fullText: text,
    hiddenChars: text.length - TERMINAL_OUTPUT_PREVIEW_CHARS,
  };
}

export type DerivedDiffFile = {
  path: string;
  changeType: string;
  /** Raw unified diff text or null if the patch carries no inline diff. */
  diff: string | null;
};

export function derivePatchsetFiles(
  patchsets: CodeUiPatchsetSnapshot[],
): DerivedDiffFile[] {
  return patchsets.flatMap((patchset) =>
    patchset.changes.map((change) => ({
      path: change.path,
      changeType: change.changeType,
      diff: change.diff ?? null,
    })),
  );
}

/**
 * Compose the chat header label out of provider + thread + status. Returns
 * `null` when the snapshot is missing so the header can render its own
 * "no session" placeholder rather than fabricating defaults.
 */
export function deriveChatHeader(
  snapshot: CodeUiSessionSnapshot | null,
): { title: string; provider: string; status: string } | null {
  if (!snapshot) return null;
  const title = snapshot.threadId ?? "Active session";
  return {
    title,
    provider: `${snapshot.provider.provider}${
      snapshot.provider.model ? ` · ${snapshot.provider.model}` : ""
    }`,
    status: snapshot.status,
  };
}
