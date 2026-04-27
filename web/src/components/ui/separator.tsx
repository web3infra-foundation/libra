/**
 * Thin divider line — a styled wrapper around Base UI's Separator primitive.
 *
 * The `data-horizontal` / `data-vertical` selectors come from the underlying
 * primitive, which sets one of those attributes based on `orientation`.
 */
"use client"

import { Separator as SeparatorPrimitive } from "@base-ui/react/separator"

import { cn } from "@/lib/utils"

/**
 * Horizontal (default) or vertical separator.
 *
 * @param orientation - Layout direction; horizontal renders as a 1px tall row,
 *                      vertical as a 1px wide column that self-stretches.
 */
function Separator({
  className,
  orientation = "horizontal",
  ...props
}: SeparatorPrimitive.Props) {
  return (
    <SeparatorPrimitive
      data-slot="separator"
      orientation={orientation}
      className={cn(
        "shrink-0 bg-border data-horizontal:h-px data-horizontal:w-full data-vertical:w-px data-vertical:self-stretch",
        className
      )}
      {...props}
    />
  )
}

export { Separator }
