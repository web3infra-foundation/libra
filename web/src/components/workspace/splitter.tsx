/**
 * Draggable splitter that resizes adjacent panels.
 *
 * The splitter itself is a thin 1px element with a wider invisible hit area
 * (±3px) so the user does not have to land precisely on the line. While
 * dragging, the body cursor is locked so the resize cursor persists even
 * when the pointer leaves the splitter rectangle, and `user-select` is
 * disabled to prevent accidental text selection.
 *
 * Drag math is delegated to the parent via `onDrag(delta, startValue)`,
 * letting the parent clamp the result against any layout-aware bounds it
 * controls (e.g. `Workspace.tsx` clamps against the chat min-width).
 */
"use client";

import { useState, type MouseEvent } from "react";

import { cn } from "@/lib/utils";

/** Vertical splitters resize horizontally (between left/right panels); horizontal splitters resize vertically. */
type Orientation = "vertical" | "horizontal";

/** Splitter props. */
type Props = {
  orientation?: Orientation;
  /** Current width/height value of the controlled panel. */
  value: number;
  /**
   * Drag callback. Receives the cumulative pixel delta from drag start and
   * the starting value, so the parent can compute `startValue ± delta`
   * and apply its own bounds.
   */
  onDrag: (delta: number, startValue: number) => void;
};

/**
 * Draggable resize handle.
 *
 * Boundary: the move/up listeners are bound to `window` rather than the
 * splitter element so a fast drag that overshoots the gutter doesn't lose
 * tracking. They are removed on mouseup to avoid leaks.
 */
export function Splitter({ orientation = "vertical", value, onDrag }: Props) {
  const [hover, setHover] = useState(false);
  const [drag, setDrag] = useState(false);
  const isVertical = orientation === "vertical";

  function handleMouseDown(e: MouseEvent) {
    e.preventDefault();
    // Capture the starting coordinate for the relevant axis once at drag
    // start; subsequent move events compute their delta against this.
    const startCoord = isVertical ? e.clientX : e.clientY;
    const startValue = value;
    setDrag(true);
    document.body.style.cursor = isVertical ? "col-resize" : "row-resize";
    document.body.style.userSelect = "none";

    function onMove(ev: globalThis.MouseEvent) {
      const current = isVertical ? ev.clientX : ev.clientY;
      onDrag(current - startCoord, startValue);
    }

    function onUp() {
      setDrag(false);
      // Reset body styles regardless of where the drag ended.
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    }

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  // Treat hover and active drag the same for visual feedback so the gutter
  // stays highlighted while dragging even if the pointer leaves it.
  const active = hover || drag;

  return (
    <div
      onMouseDown={handleMouseDown}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      className={cn(
        "relative z-[5] shrink-0 bg-rule",
        isVertical ? "w-px cursor-col-resize" : "h-px cursor-row-resize",
      )}
    >
      <div
        className={cn(
          "absolute",
          isVertical ? "-left-[3px] -right-[3px] top-0 bottom-0" : "-top-[3px] -bottom-[3px] left-0 right-0",
        )}
      />
      <div
        className={cn(
          "absolute inset-0 transition-colors duration-150",
          active ? "bg-accent" : "bg-transparent",
        )}
      />
    </div>
  );
}
