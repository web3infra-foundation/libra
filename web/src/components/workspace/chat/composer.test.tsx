// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Composer } from "./composer";

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

describe("Composer", () => {
  it("does not render a static file-context fixture chip", () => {
    const { container, unmount } = render(<Composer onSubmit={vi.fn()} />);

    expect(container.textContent).toContain("Add context");
    expect(container.textContent).not.toContain("src/lib/query.ts");
    const contextButton = Array.from(container.querySelectorAll("button"))
      .find((button) => button.textContent?.includes("Add context"));
    expect(contextButton?.disabled).toBe(true);

    unmount();
  });
});
