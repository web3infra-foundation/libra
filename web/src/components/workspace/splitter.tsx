"use client";

import { useState, type MouseEvent } from "react";

import { cn } from "@/lib/utils";

type Orientation = "vertical" | "horizontal";

type Props = {
  orientation?: Orientation;
  value: number;
  onDrag: (delta: number, startValue: number) => void;
};

export function Splitter({ orientation = "vertical", value, onDrag }: Props) {
  const [hover, setHover] = useState(false);
  const [drag, setDrag] = useState(false);
  const isVertical = orientation === "vertical";

  function handleMouseDown(e: MouseEvent) {
    e.preventDefault();
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
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    }

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

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
