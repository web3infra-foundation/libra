/**
 * Top-level three-pane workspace layout.
 *
 * Renders, left → right: sidebar (threads), chat (conversation + composer +
 * embedded terminal), and workflow (pipeline / summary / diff tabs). Two
 * vertical {@link Splitter}s let the user resize the side panes; the chat
 * pane in the middle takes the remaining space.
 *
 * Both splitter widths persist to localStorage via {@link useStoredNumber}
 * so the layout survives reload. Drag handlers clamp against:
 *   - hard min/max for that pane,
 *   - and a dynamic max so the chat pane never collapses below `CHAT_MIN`.
 */
"use client";

import { Chat } from "./chat/chat";
import { Sidebar } from "./sidebar/sidebar";
import { Splitter } from "./splitter";
import { Workflow } from "./workflow/workflow";
import { useStoredNumber } from "@/lib/persisted-state";
import { clamp } from "@/lib/storage";

// Layout bounds, in CSS pixels. These are tuned for the visual density of
// the workspace; the chat min in particular ensures the composer remains
// usable on narrow viewports.
const SIDEBAR_MIN = 180;
const SIDEBAR_MAX = 420;
const WORKFLOW_MIN = 420;
const WORKFLOW_MAX = 980;
const CHAT_MIN = 360;

/**
 * The full workspace. State-only component; no async data fetching here —
 * see the chat/workflow modules for their own data plumbing.
 */
export function Workspace() {
  const [sidebarW, setSidebarW] = useStoredNumber("libra.sidebarW", 248);
  const [workflowW, setWorkflowW] = useStoredNumber("libra.workflowW", 660);

  // Sidebar resize: drag right → wider. The dynamic upper bound is the
  // viewport width minus the workflow pane and the chat minimum, so the
  // user can't drag the sidebar so wide that the chat collapses.
  function onDragSidebar(dx: number, startW: number) {
    const total = window.innerWidth;
    const max = Math.min(SIDEBAR_MAX, total - workflowW - CHAT_MIN);
    setSidebarW(clamp(startW + dx, SIDEBAR_MIN, max));
  }

  // Workflow resize: drag right → narrower (the splitter sits to the left
  // of the workflow pane), so the delta is subtracted.
  function onDragWorkflow(dx: number, startW: number) {
    const total = window.innerWidth;
    const max = Math.min(WORKFLOW_MAX, total - sidebarW - CHAT_MIN);
    setWorkflowW(clamp(startW - dx, WORKFLOW_MIN, max));
  }

  return (
    <div className="flex h-screen w-full">
      <Sidebar width={sidebarW} />
      <Splitter value={sidebarW} onDrag={onDragSidebar} />
      <Chat />
      <Splitter value={workflowW} onDrag={onDragWorkflow} />
      <Workflow width={workflowW} />
    </div>
  );
}
