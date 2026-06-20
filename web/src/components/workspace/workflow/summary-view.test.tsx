// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const storeState = vi.hoisted(() => ({
  snapshot: null as unknown,
  status: null as unknown,
  refreshStatus: vi.fn(async () => undefined),
}));

vi.mock("@/lib/code-ui/store", () => ({
  useCodeUiStore: () => storeState,
}));

import { SummaryView } from "./summary-view";

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
  storeState.status = null;
  storeState.refreshStatus.mockClear();
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("SummaryView", () => {
  it("refreshes repo status from the live store API", () => {
    storeState.status = {
      head: { type: "branch", name: "main" },
      has_commits: true,
      upstream: null,
      staged: { new: [], modified: [], deleted: [] },
      unstaged: { modified: [], deleted: [] },
      untracked: [],
      ignored: [],
      is_clean: true,
      stash_entries: 0,
    };

    const { container, unmount } = render(<SummaryView />);
    const refreshButton = Array.from(container.querySelectorAll("button")).find((button) =>
      button.textContent?.includes("refresh"),
    );

    expect(refreshButton).toBeDefined();
    act(() => {
      refreshButton?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(storeState.refreshStatus).toHaveBeenCalledTimes(1);

    unmount();
  });
});
