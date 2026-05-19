// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const storeState = vi.hoisted(() => ({
  snapshot: null as unknown,
}));
const controllerState = vi.hoisted(() => ({
  value: {
    respond: vi.fn(),
    status: { kind: "idle" },
  },
}));

vi.mock("@/lib/code-ui/store", () => ({
  useCodeUiStore: () => ({ snapshot: storeState.snapshot }),
}));

vi.mock("@/lib/code-ui/controller", () => ({
  useBrowserController: () => controllerState.value,
}));

import type {
  CodeUiInteractionKind,
  CodeUiInteractionRequest,
  CodeUiSessionSnapshot,
} from "@/lib/code-ui/types";

import { InteractionPanel } from "./interaction-panel";

function baseSnapshot(interaction: CodeUiInteractionRequest): CodeUiSessionSnapshot {
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
      interactiveApprovals: true,
      structuredQuestions: true,
      providerSessionResume: false,
    },
    controller: { kind: "browser", canWrite: true, loopbackOnly: true },
    status: "awaiting_interaction",
    transcript: [],
    plans: [],
    tasks: [],
    toolCalls: [],
    patchsets: [],
    interactions: [interaction],
    updatedAt: "2026-05-14T00:00:00Z",
  };
}

function interaction(kind: CodeUiInteractionKind): CodeUiInteractionRequest {
  return {
    id: `${kind}-1`,
    kind,
    title: `${kind} title`,
    description: `${kind} description`,
    prompt: kind === "request_user_input" ? "Which environment?" : undefined,
    options:
      kind === "request_user_input"
        ? []
        : [
            { id: "allow", label: "Allow" },
            { id: "deny", label: "Deny" },
          ],
    status: "pending",
    metadata:
      kind === "request_user_input"
        ? {
            questions: [
              {
                id: "environment",
                prompt: "Which environment?",
                kind: "single",
                options: [{ id: "dev", label: "Dev" }],
              },
            ],
          }
        : {},
    requestedAt: "2026-05-14T00:00:00Z",
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
  controllerState.value = {
    respond: vi.fn(),
    status: { kind: "idle" },
  };
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("InteractionPanel", () => {
  it.each([
    ["intent_review_choice", "Intent review", "Allow"],
    ["post_plan_choice", "Plan choice", "Allow"],
    ["approval", "Approval", "Apply to future:"],
    ["sandbox_approval", "Sandbox approval", "Apply to future:"],
    ["request_user_input", "User input", "Submit answers"],
  ] satisfies Array<[CodeUiInteractionKind, string, string]>)(
    "renders pending %s controls",
    (kind, label, controlText) => {
      storeState.snapshot = baseSnapshot(interaction(kind));

      const { container, unmount } = render(<InteractionPanel />);

      expect(container.textContent).toContain(label);
      expect(container.textContent).toContain(`${kind} title`);
      expect(container.textContent).toContain(controlText);

      unmount();
    },
  );

  it("disables interaction controls for a read-only controller", () => {
    const snapshot = baseSnapshot(interaction("intent_review_choice"));
    snapshot.controller = {
      kind: "automation",
      canWrite: false,
      loopbackOnly: true,
      ownerLabel: "scenario",
    };
    storeState.snapshot = snapshot;

    const { container, unmount } = render(<InteractionPanel />);
    const firstButton = container.querySelector("button");

    expect(firstButton).not.toBeNull();
    expect(firstButton?.disabled).toBe(true);

    unmount();
  });

  it("surfaces browser control errors from the controller hook", () => {
    storeState.snapshot = baseSnapshot(interaction("approval"));
    controllerState.value = {
      respond: vi.fn(),
      status: {
        kind: "error",
        code: "BROWSER_CONTROL_DISABLED",
        message: "restart with --browser-control loopback",
      },
    };

    const { container, unmount } = render(<InteractionPanel />);

    expect(container.textContent).toContain("BROWSER_CONTROL_DISABLED");
    expect(container.textContent).toContain("restart with --browser-control loopback");

    unmount();
  });
});
