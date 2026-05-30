import { describe, expect, it } from "vitest";

import {
  CHAT_MESSAGE_PREVIEW_CHARS,
  CHAT_TRANSCRIPT_MESSAGE_LIMIT,
  deriveChatMessages,
  deriveTerminalRows,
} from "./view-model";
import type { CodeUiSessionSnapshot, CodeUiTranscriptEntry } from "./types";

function transcriptEntry(index: number, content = `message ${index}`): CodeUiTranscriptEntry {
  return {
    id: `entry-${index}`,
    kind: index % 2 === 0 ? "user_message" : "assistant_message",
    content,
    streaming: false,
    metadata: {},
    createdAt: "2026-05-14T00:00:00Z",
    updatedAt: "2026-05-14T00:00:00Z",
  };
}

function snapshot(transcript: CodeUiTranscriptEntry[]): CodeUiSessionSnapshot {
  return {
    sessionId: "session-1",
    threadId: "thread-1",
    workingDir: "/repo",
    provider: { provider: "test" },
    capabilities: {
      messageInput: true,
      streamingText: true,
      planUpdates: false,
      toolCalls: false,
      patchsets: false,
      interactiveApprovals: false,
      structuredQuestions: false,
      providerSessionResume: false,
    },
    controller: { kind: "browser", canWrite: true, loopbackOnly: true },
    status: "idle",
    transcript,
    plans: [],
    tasks: [],
    toolCalls: [],
    patchsets: [],
    interactions: [],
    updatedAt: "2026-05-14T00:00:00Z",
  };
}

describe("deriveChatMessages", () => {
  it("collapses earlier transcript entries before mapping the chat pane", () => {
    const transcript = Array.from(
      { length: CHAT_TRANSCRIPT_MESSAGE_LIMIT + 3 },
      (_, index) => transcriptEntry(index),
    );

    const messages = deriveChatMessages(snapshot(transcript));

    expect(messages).toHaveLength(CHAT_TRANSCRIPT_MESSAGE_LIMIT + 1);
    expect(messages[0]).toMatchObject({
      kind: "info_note",
      title: "transcript",
      body: "Transcript collapsed: 3 earlier entries hidden.",
    });
    expect(messages[1].id).toBe("entry-3");
  });

  it("truncates oversized message bodies until the user expands them", () => {
    const tail = "tail-after-expand";
    const longBody = `${"x".repeat(CHAT_MESSAGE_PREVIEW_CHARS + 8)}${tail}`;

    const messages = deriveChatMessages(snapshot([transcriptEntry(1, longBody)]));

    expect(messages[0].body).toHaveLength(CHAT_MESSAGE_PREVIEW_CHARS);
    expect(messages[0].body).not.toContain(tail);
    expect(messages[0].fullBody).toBe(longBody);
    expect(messages[0].hiddenChars).toBe(8 + tail.length);
  });
});

describe("deriveTerminalRows", () => {
  it("maps completed backend tool calls to passed terminal rows", () => {
    const state = snapshot([]);
    state.toolCalls = [
      {
        id: "tool-1",
        toolName: "shell",
        status: "completed",
        summary: "cargo test",
        details: "ok",
        updatedAt: "2026-05-14T00:00:00Z",
      },
    ];

    const toolRow = deriveTerminalRows(state).find((row) =>
      row.text.includes("cargo test"),
    );
    expect(toolRow?.kind).toBe("pass");
  });
});
