// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const storeState = vi.hoisted(() => ({
  snapshot: null as unknown,
  status: null as unknown,
}));
const controllerState = vi.hoisted(() => ({
  cancel: vi.fn(),
}));

vi.mock("@/lib/code-ui/store", () => ({
  useCodeUiStore: () => storeState,
}));
vi.mock("@/lib/code-ui/controller", () => ({
  useBrowserController: () => controllerState,
}));

import type { CodeUiSessionSnapshot } from "@/lib/code-ui/types";

import { Workflow } from "./workflow";

type ActGlobal = typeof globalThis & {
  IS_REACT_ACT_ENVIRONMENT?: boolean;
};

function baseSnapshot(): CodeUiSessionSnapshot {
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
  (globalThis as ActGlobal).IS_REACT_ACT_ENVIRONMENT = true;
  storeState.snapshot = null;
  storeState.status = null;
  controllerState.cancel.mockReset();
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("Workflow", () => {
  it("renders tasks as first-class workflow rows with snapshot details", () => {
    const snapshot = baseSnapshot();
    snapshot.tasks = [
      {
        id: "task-42",
        title: "Pin snapshot projection",
        status: "in_progress",
        details: "Task details from the live runtime snapshot.",
        updatedAt: "2026-05-14T00:00:00Z",
      },
    ];
    storeState.snapshot = snapshot;

    const { container, unmount } = render(<Workflow width={520} />);

    expect(container.textContent).toContain("Tasks");
    expect(container.textContent).toContain("task-42");
    expect(container.textContent).toContain("Pin snapshot projection");
    expect(container.textContent).toContain("RUNNING");

    const taskButton = Array.from(container.querySelectorAll("button")).find((button) =>
      button.textContent?.includes("Pin snapshot projection"),
    );
    expect(taskButton).toBeDefined();

    act(() => {
      taskButton?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(container.textContent).toContain("Task ID");
    expect(container.textContent).toContain("Task details from the live runtime snapshot.");

    unmount();
  });
});
