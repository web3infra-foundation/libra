// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const storeState = vi.hoisted(() => ({
  snapshot: null as unknown,
  connection: { kind: "ready" },
}));

vi.mock("@/lib/code-ui/store", () => ({
  useCodeUiStore: () => storeState,
}));

import type { CodeUiSessionSnapshot } from "@/lib/code-ui/types";
import { TERMINAL_OUTPUT_PREVIEW_CHARS } from "@/lib/code-ui/view-model";

import { Terminal } from "./terminal";

function baseSnapshot(): CodeUiSessionSnapshot {
  return {
    sessionId: "session-1",
    threadId: "thread-1",
    workingDir: "/repo",
    provider: { provider: "test", model: "fixture" },
    capabilities: {
      messageInput: true,
      streamingText: true,
      planUpdates: false,
      toolCalls: true,
      patchsets: false,
      interactiveApprovals: false,
      structuredQuestions: false,
      providerSessionResume: false,
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

function render(node: React.ReactNode) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root: Root | null = null;
  act(() => {
    root = createRoot(container);
    root.render(node);
  });
  return {
    container,
    unmount() {
      act(() => root?.unmount());
      container.remove();
    },
  };
}

beforeEach(() => {
  storeState.snapshot = null;
  storeState.connection = { kind: "ready" };
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("Terminal", () => {
  it("truncates long tool output and expands it on demand", () => {
    const snapshot = baseSnapshot();
    const tail = "visible-tail-after-expansion";
    snapshot.toolCalls = [
      {
        id: "tool-1",
        toolName: "shell",
        status: "succeeded",
        summary: "cargo test",
        details: `${"x".repeat(TERMINAL_OUTPUT_PREVIEW_CHARS + 24)}${tail}`,
        updatedAt: "2026-05-14T00:00:00Z",
      },
    ];
    storeState.snapshot = snapshot;

    const { container, unmount } = render(<Terminal height={240} onClose={() => {}} />);

    expect(container.textContent).toContain("cargo test");
    expect(container.textContent).toContain("Show full output");
    expect(container.textContent).not.toContain(tail);

    const expandButton = Array.from(container.querySelectorAll("button"))
      .find((button) => button.textContent?.includes("Show full output"));
    expect(expandButton).toBeDefined();

    act(() => {
      expandButton?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(container.textContent).toContain(tail);
    expect(container.textContent).toContain("Show less");

    unmount();
  });
});
