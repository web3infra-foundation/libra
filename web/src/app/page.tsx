/**
 * Home route (`/`) for the Libra web UI.
 *
 * Renders the full {@link Workspace} layout — sidebar, chat pane, workflow
 * pane, terminal — which is the only screen this static export ships.
 */
import type { Metadata } from "next";

import { Workspace } from "@/components/workspace/workspace";

/** Page-specific metadata; overrides the template defined in the root layout. */
export const metadata: Metadata = {
  title: "Libra — Agent Workspace",
};

/**
 * Top-level page component.
 *
 * Stateless wrapper around {@link Workspace}; all state and resizable panel
 * logic live inside that component.
 */
export default function Home() {
  return <Workspace />;
}
