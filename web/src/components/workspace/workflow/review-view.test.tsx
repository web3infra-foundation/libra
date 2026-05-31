// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const storeState = vi.hoisted(() => ({
  snapshot: null as unknown,
}));

vi.mock("@/lib/code-ui/store", () => ({
  useCodeUiStore: () => ({ snapshot: storeState.snapshot }),
}));

import type { CodeUiSessionSnapshot } from "@/lib/code-ui/types";

import { ReviewView } from "./review-view";

function baseSnapshot(): CodeUiSessionSnapshot {
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
      patchsets: true,
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
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ReviewView", () => {
  it("renders an empty review state when there is no active session", () => {
    const { container, unmount } = render(<ReviewView />);

    expect(container.textContent).toContain("No PatchSet diffs to review yet.");

    unmount();
  });

  it("renders empty diff fallback text", () => {
    const snapshot = baseSnapshot();
    snapshot.patchsets = [
      {
        id: "patch-1",
        status: "ready",
        updatedAt: "2026-05-14T00:00:00Z",
        changes: [{ path: "empty.txt", changeType: "modified" }],
      },
    ];
    storeState.snapshot = snapshot;

    const { container, unmount } = render(<ReviewView />);

    expect(container.textContent).toContain("PatchSet patch-1");
    expect(container.textContent).toContain("ready · 1 files · +0 −0");
    expect(container.textContent).toContain("empty.txt");
    expect(container.textContent).toContain("No inline diff for this change.");

    unmount();
  });

  it("renders patchset grouping and keeps duplicate file paths distinct", () => {
    const snapshot = baseSnapshot();
    snapshot.patchsets = [
      {
        id: "patch-a",
        status: "ready",
        updatedAt: "2026-05-14T00:00:00Z",
        changes: [
          {
            path: "shared.txt",
            changeType: "modified",
            diff: ["@@ -1 +1 @@", "-old", "+new"].join("\n"),
          },
        ],
      },
      {
        id: "patch-b",
        status: "released",
        updatedAt: "2026-05-14T00:00:00Z",
        changes: [
          {
            path: "shared.txt",
            changeType: "modified",
            diff: ["@@ -1 +1 @@", "-alpha", "+beta"].join("\n"),
          },
        ],
      },
    ];
    storeState.snapshot = snapshot;

    const { container, unmount } = render(<ReviewView />);

    expect(container.textContent).toContain("2 PatchSets");
    expect(container.textContent).toContain("PatchSet patch-a");
    expect(container.textContent).toContain("PatchSet patch-b");
    expect(container.textContent).toContain("ready · 1 files · +1 −1");
    expect(container.textContent).toContain("released · 1 files · +1 −1");
    expect(container.querySelectorAll("button").length).toBeGreaterThanOrEqual(3);
    expect(container.textContent).toContain("shared.txt");
    expect(container.textContent).toContain("old");
    expect(container.textContent).toContain("alpha");

    unmount();
  });

  it("fails open when diff parsing fails", () => {
    const snapshot = baseSnapshot();
    snapshot.patchsets = [
      {
        id: "patch-1",
        status: "ready",
        updatedAt: "2026-05-14T00:00:00Z",
        changes: [
          {
            path: "broken.diff",
            changeType: "modified",
            diff: "not a unified diff",
          },
        ],
      },
    ];
    storeState.snapshot = snapshot;

    const { container, unmount } = render(<ReviewView />);

    expect(container.textContent).toContain("unable to parse diff");
    expect(container.textContent).toContain("not a unified diff");

    unmount();
  });

  it("collapses long parsed diffs after the render limit", () => {
    const snapshot = baseSnapshot();
    const lines = Array.from({ length: 350 }, (_, i) => `+line ${i}`);
    snapshot.patchsets = [
      {
        id: "patch-1",
        status: "ready",
        updatedAt: "2026-05-14T00:00:00Z",
        changes: [
          {
            path: "long.diff",
            changeType: "modified",
            diff: ["@@ -1,1 +1,350 @@", ...lines].join("\n"),
          },
        ],
      },
    ];
    storeState.snapshot = snapshot;

    const { container, unmount } = render(<ReviewView />);

    expect(container.textContent).toContain("line 0");
    expect(container.textContent).toContain("line 299");
    expect(container.textContent).not.toContain("line 300");
    expect(container.textContent).toContain("Diff collapsed: 50 more diff lines hidden.");

    unmount();
  });
});
