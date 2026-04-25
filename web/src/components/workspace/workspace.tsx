"use client";

import { Chat } from "./chat/chat";
import { Sidebar } from "./sidebar/sidebar";
import { Splitter } from "./splitter";
import { Workflow } from "./workflow/workflow";
import { useStoredNumber } from "@/lib/persisted-state";
import { clamp } from "@/lib/storage";

const SIDEBAR_MIN = 180;
const SIDEBAR_MAX = 420;
const WORKFLOW_MIN = 420;
const WORKFLOW_MAX = 980;
const CHAT_MIN = 360;

export function Workspace() {
  const [sidebarW, setSidebarW] = useStoredNumber("libra.sidebarW", 248);
  const [workflowW, setWorkflowW] = useStoredNumber("libra.workflowW", 660);

  function onDragSidebar(dx: number, startW: number) {
    const total = window.innerWidth;
    const max = Math.min(SIDEBAR_MAX, total - workflowW - CHAT_MIN);
    setSidebarW(clamp(startW + dx, SIDEBAR_MIN, max));
  }

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
