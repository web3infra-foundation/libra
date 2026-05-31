// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { DetailPanel } from "./detail-panel";
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
      <DetailPanel detail={detail} onClose={() => {}} snapshot={null} />,
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
      <DetailPanel detail={detail} onClose={() => {}} snapshot={null} />,
    );

    expect(container.textContent).toContain("actual tool output from the snapshot");
    expect(container.textContent).not.toContain("cargo test --lib optimistic");
    expect(container.textContent).not.toContain("src/lib/query.ts");

    unmount();
  });
});
