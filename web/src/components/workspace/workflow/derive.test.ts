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

  it("preserves live plan metadata on the projected workflow plans", () => {
    const state = snapshot();
    state.plans = [
      {
        id: "plan-1",
        title: "Execution plan",
        summary: "Apply the live session snapshot to the workflow pane.",
        status: "running",
        steps: [
          { step: "inspect", status: "running" },
          { step: "verify", status: "queued" },
        ],
        updatedAt: "2026-05-14T00:00:00Z",
      },
      {
        id: "plan-2",
        title: "Test plan",
        summary: "Confirm the projection keeps the metadata visible.",
        status: "queued",
        steps: [],
        updatedAt: "2026-05-14T00:00:00Z",
      },
    ];

    const workflow = deriveWorkflow(state);
    expect(workflow.plans.execution).toMatchObject({
      id: "plan-1",
      title: "Execution plan",
      summary: "Apply the live session snapshot to the workflow pane.",
    });
    expect(workflow.plans.test).toMatchObject({
      id: "plan-2",
      title: "Test plan",
      summary: "Confirm the projection keeps the metadata visible.",
    });
  });
});
