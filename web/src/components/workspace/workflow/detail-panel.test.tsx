// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { DetailPanel } from "./detail-panel";
import type { CodeUiSessionSnapshot } from "@/lib/code-ui/types";
import type { DetailState } from "./types";

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

afterEach(() => {
  document.body.innerHTML = "";
});

describe("DetailPanel", () => {
  it("renders intent details from the live view model without demo fixture text", () => {
    const detail: DetailState = {
      kind: "intent",
      data: {
        title: "Audit Code UI",
        revision: "thread abc12345",
        summary: "Use the current session snapshot as the only source of truth.",
        constraints: ["Do not show mock workflow content."],
        confirmed: true,
      },
    };

    const { container, unmount } = render(
      <DetailPanel detail={detail} snapshot={null} onClose={() => {}} />,
    );

    expect(container.textContent).toContain("Audit Code UI");
    expect(container.textContent).toContain("Do not show mock workflow content.");
    expect(container.textContent).not.toContain("useMutation");
    expect(container.textContent).not.toContain("src/hooks/useMutation.ts");

    unmount();
  });

  it("renders run output from tool details without fabricated command output", () => {
    const detail: DetailState = {
      kind: "run",
      data: {
        id: "tool-1",
        step: "read_file",
        result: "pass",
        ago: "0s",
        label: "Read web README",
        details: "actual tool output from the snapshot",
      },
    };

    const { container, unmount } = render(
      <DetailPanel detail={detail} snapshot={null} onClose={() => {}} />,
    );

    expect(container.textContent).toContain("actual tool output from the snapshot");
    expect(container.textContent).not.toContain("cargo test --lib optimistic");
    expect(container.textContent).not.toContain("src/lib/query.ts");

    unmount();
  });

  it("collapses oversized task and run details until explicitly expanded", () => {
    const longTaskDetails = `task-start\n${"task-line\n".repeat(2600)}task-end`;
    const longRunDetails = `run-start\n${"run-line\n".repeat(2600)}run-end`;

    const taskView = render(
      <DetailPanel
        detail={{
          kind: "task",
          data: {
            id: "task-1",
            title: "Long task",
            status: "running",
            ago: "1m",
            details: longTaskDetails,
          },
        }}
        snapshot={null}
        onClose={() => {}}
      />,
    );

    const taskToggle = Array.from(taskView.container.querySelectorAll("button")).find((button) =>
      button.textContent?.includes("Show full output"),
    );
    expect(taskView.container.textContent).toContain("Show full output");
    expect(taskView.container.textContent).not.toContain("task-end");
    act(() => {
      taskToggle?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(taskView.container.textContent).toContain("task-end");
    taskView.unmount();

    const runView = render(
      <DetailPanel
        detail={{
          kind: "run",
          data: {
            id: "run-1",
            step: "tool",
            result: "pass",
            ago: "1m",
            label: "Long run",
            details: longRunDetails,
          },
        }}
        snapshot={null}
        onClose={() => {}}
      />,
    );

    const runToggle = Array.from(runView.container.querySelectorAll("button")).find((button) =>
      button.textContent?.includes("Show full output"),
    );
    expect(runView.container.textContent).toContain("Show full output");
    expect(runView.container.textContent).not.toContain("run-end");
    act(() => {
      runToggle?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(runView.container.textContent).toContain("run-end");
    runView.unmount();
  });

  it("renders validation and release details from the live snapshot without placeholder identities", () => {
    const snapshot: CodeUiSessionSnapshot = {
      sessionId: "session-1",
      threadId: "thread-abc12345",
      workingDir: "/workspace/libra",
      provider: {
        provider: "ollama",
        model: "kimi-k2.6:cloud",
        mode: "web-only",
        managed: true,
      },
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
      controller: {
        kind: "browser",
        ownerLabel: "browser",
        canWrite: true,
        leaseExpiresAt: "2026-05-31T12:00:00Z",
        reason: "active browser lease",
        loopbackOnly: true,
      },
      status: "executing_tool",
      transcript: [],
      plans: [],
      tasks: [],
      toolCalls: [
        {
          id: "tool-1",
          toolName: "update_plan",
          status: "completed",
          summary: "Update plan",
          details: "applied",
          updatedAt: "2026-05-31T11:58:00Z",
        },
      ],
      patchsets: [
        {
          id: "patch-1",
          status: "applied",
          changes: [],
          updatedAt: "2026-05-31T11:58:30Z",
        },
      ],
      interactions: [
        {
          id: "interaction-1",
          kind: "approval",
          options: [],
          status: "pending",
          metadata: {},
          requestedAt: "2026-05-31T11:57:00Z",
        },
      ],
      updatedAt: "2026-05-31T11:59:00Z",
    };

    const validation = {
      kind: "validation",
    } as const satisfies DetailState;
    const release = {
      kind: "release",
    } as const satisfies DetailState;

    const validationView = render(
      <DetailPanel detail={validation} snapshot={snapshot} onClose={() => {}} />,
    );
    expect(validationView.container.textContent).toContain("executing_tool");
    expect(validationView.container.textContent).toContain("1");
    expect(validationView.container.textContent).not.toContain("web3infra/default");
    expect(validationView.container.textContent).not.toContain("erin@web3infra");
    validationView.unmount();

    const releaseView = render(
      <DetailPanel detail={release} snapshot={snapshot} onClose={() => {}} />,
    );
    expect(releaseView.container.textContent).toContain("ollama");
    expect(releaseView.container.textContent).toContain("/workspace/libra");
    expect(releaseView.container.textContent).not.toContain("web3infra/default");
    expect(releaseView.container.textContent).not.toContain("erin@web3infra");
    releaseView.unmount();
  });
});
