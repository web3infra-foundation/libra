// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { SettingsMenu } from "./settings-menu";

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

describe("SettingsMenu", () => {
  it("renders a neutral local-session menu instead of a hardcoded personal account", () => {
    const { container, unmount } = render(<SettingsMenu />);

    expect(container.textContent).toContain("Local session");
    expect(container.textContent).toContain("loopback-only");
    expect(container.textContent).not.toContain("Erin Chen");
    expect(container.textContent).not.toContain("erin@web3infra.io");

    unmount();
  });
});
