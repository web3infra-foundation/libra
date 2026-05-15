// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { Message } from "./message";

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

describe("Message", () => {
  it("renders streaming assistant messages with a live indicator", () => {
    const { container, unmount } = render(
      <Message
        message={{
          id: "assistant-1",
          role: "assistant",
          time: "12:00",
          body: "Working on it",
          streaming: true,
          title: "Plan update",
        }}
      />,
    );

    expect(container.textContent).toContain("streaming");
    expect(container.textContent).toContain("Working on it");
    expect(container.querySelector(".libra-caret")).not.toBeNull();

    unmount();
  });

  it("expands truncated assistant messages on demand", () => {
    const { container, unmount } = render(
      <Message
        message={{
          id: "assistant-2",
          role: "assistant",
          time: "12:01",
          body: "preview body",
          fullBody: "preview body plus the hidden tail",
          hiddenChars: 21,
        }}
      />,
    );

    expect(container.textContent).toContain("preview body");
    expect(container.textContent).not.toContain("hidden tail");

    const expandButton = Array.from(container.querySelectorAll("button"))
      .find((button) => button.textContent?.includes("Show full message"));
    expect(expandButton).toBeDefined();

    act(() => {
      expandButton?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(container.textContent).toContain("hidden tail");
    expect(container.textContent).toContain("Show less");

    unmount();
  });
});
