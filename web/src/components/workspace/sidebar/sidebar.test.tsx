// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const storeState = vi.hoisted(() => ({
  snapshot: null as unknown,
  repo: null as unknown,
  status: null as unknown,
  threads: [] as unknown[],
  connection: { kind: "ready" as const },
  refreshThreads: vi.fn(async () => undefined),
}));

vi.mock("@/lib/code-ui/store", () => ({
  useCodeUiStore: () => storeState,
}));

import { Sidebar } from "./sidebar";

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
  storeState.repo = null;
  storeState.status = null;
  storeState.threads = [];
  storeState.connection = { kind: "ready" };
  storeState.refreshThreads.mockClear();
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("Sidebar", () => {
  it("refreshes the thread list from the live store API", () => {
    storeState.repo = {
      name: "libra",
      head: "main",
      branch: "main",
      description: null,
      root: "/repo",
    };
    storeState.status = null;
    storeState.threads = [
      {
        id: "thread-1",
        title: "Audit web UI",
        archived: false,
        currentIntentId: null,
        createdAt: "2026-05-14T00:00:00Z",
        updatedAt: "2026-05-14T00:01:00Z",
      },
    ];

    const { container, unmount } = render(<Sidebar width={320} />);
    const refreshButton = Array.from(container.querySelectorAll("button")).find((button) =>
      button.getAttribute("aria-label") === "Refresh thread list",
    );

    expect(refreshButton).toBeDefined();
    act(() => {
      refreshButton?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(storeState.refreshThreads).toHaveBeenCalledTimes(1);

    unmount();
  });
});
