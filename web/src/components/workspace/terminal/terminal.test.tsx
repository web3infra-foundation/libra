// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const storeState = vi.hoisted(() => ({
  snapshot: null as unknown,
  connection: { kind: "ready" },
}));
const diagnosticsState = vi.hoisted(() => ({
  value: null as unknown,
}));

vi.mock("@/lib/code-ui/store", () => ({
  useCodeUiStore: () => storeState,
}));
vi.mock("@/lib/code-ui/client", () => ({
  getDiagnostics: vi.fn(() => {
    if (diagnosticsState.value) return Promise.resolve(diagnosticsState.value);
    return Promise.reject(new Error("diagnostics unavailable"));
  }),
}));

import type { CodeUiDiagnostics, CodeUiSessionSnapshot } from "@/lib/code-ui/types";
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
  diagnosticsState.value = null;
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

  it("renders diagnostics as best-effort agent rows", async () => {
    storeState.snapshot = baseSnapshot();
    diagnosticsState.value = {
      pid: 4321,
      provider: "test",
      model: "fixture",
      threadId: "thread-1",
      status: "running",
      controller: { kind: "browser", canWrite: true, loopbackOnly: true },
      ports: { web: 3867, mcp: 3868 },
      logFile: "/tmp/libra-code.log",
      activeInteractionId: "approval-1",
      lastError: "tool failed",
    } satisfies CodeUiDiagnostics;

    const { container, unmount } = render(<Terminal height={240} onClose={() => {}} />);

    await act(async () => {
      await Promise.resolve();
    });

    expect(container.textContent).toContain("diagnostics: pid 4321");
    expect(container.textContent).toContain("web 3867");
    expect(container.textContent).toContain("mcp 3868");
    expect(container.textContent).toContain("log /tmp/libra-code.log");
    expect(container.textContent).toContain("active interaction approval-1");
    expect(container.textContent).toContain("last error: tool failed");

    unmount();
  });
});
