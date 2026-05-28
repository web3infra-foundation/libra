import { describe, expect, it } from "vitest";

import type { CodeUiSessionSnapshot } from "@/lib/code-ui/types";

import { deriveWorkflow } from "./derive";

function snapshot(): CodeUiSessionSnapshot {
  return {
    sessionId: "session-1",
    threadId: "thread-1",
    workingDir: "/repo",
    provider: { provider: "test", model: "fixture" },
    capabilities: {
      messageInput: true,
      streamingText: true,
      planUpdates: true,
      toolCalls: true,
      patchsets: true,
      interactiveApprovals: true,
      structuredQuestions: true,
      providerSessionResume: true,
    },
    controller: { kind: "browser", canWrite: true, loopbackOnly: true },
    status: "idle",
    transcript: [],
    plans: [],
    tasks: [],
    toolCalls: [],
    patchsets: [],
    interactions: [],
    updatedAt: "2026-05-14T00:00:00Z",
  };
}

describe("deriveWorkflow", () => {
  it("maps completed backend tool calls to passed execution runs", () => {
    const state = snapshot();
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

    expect(deriveWorkflow(state).runs[0]).toMatchObject({
      id: "tool-1",
      result: "pass",
      label: "cargo test",
    });
  });
});
