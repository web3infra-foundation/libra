/**
 * Centralised icon library.
 *
 * Defines a tiny `<Icon>` wrapper that normalises stroke width, size, and
 * dash/cap behaviour, then exports one named arrow-function component per
 * glyph (`IconPlus`, `IconSearch`, …). All icons inherit `currentColor`
 * stroke so they re-tint via Tailwind text utilities.
 *
 * Keeping the icons in a single file is intentional: it keeps SVG payloads
 * tree-shakeable while giving every component a stable import path.
 */
import type { ReactNode, SVGProps } from "react";

/**
 * Icon props.
 *
 * - `size` – width/height in px; defaults to 16 to match small UI controls.
 * - `sw`   – stroke width; defaults to 1.5 so glyphs read crisply at
 *            small sizes without looking heavy at larger ones.
 *
 * Any other valid SVG attribute (className, onClick, aria-label) passes
 * through to the underlying `<svg>`.
 */
type IconProps = SVGProps<SVGSVGElement> & {
  size?: number;
  sw?: number;
};

/**
 * Internal SVG wrapper used by every exported icon.
 *
 * Sets sensible defaults (`fill: none`, `stroke: currentColor`, rounded
 * caps/joins) so individual glyph definitions only need their path data.
 */
function Icon({
  size = 16,
  sw = 1.5,
  fill = "none",
  stroke = "currentColor",
  children,
  ...rest
}: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill={fill}
      stroke={stroke}
      strokeWidth={sw}
      strokeLinecap="round"
      strokeLinejoin="round"
      {...rest}
    >
      {children}
    </svg>
  );
}

/** "+" plus / new-item icon. */
export const IconPlus = (p: IconProps) => (
  <Icon {...p}>
    <path d="M12 5v14M5 12h14" />
  </Icon>
);

/** Magnifying-glass / search affordance. */
export const IconSearch = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="11" cy="11" r="7" />
    <path d="m20 20-3.5-3.5" />
  </Icon>
);

/** Gear icon used for the settings menu trigger. */
export const IconSettings = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
  </Icon>
);

/** Speech-bubble shape used to denote a thread/conversation. */
export const IconThread = (p: IconProps) => (
  <Icon {...p}>
    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
  </Icon>
);

/** Git-branch glyph used in branch chips and timeline lanes. */
export const IconBranch = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="6" cy="5" r="2" />
    <circle cx="6" cy="19" r="2" />
    <circle cx="18" cy="12" r="2" />
    <path d="M6 7v10M6 12h8a2 2 0 0 0 2-2V7" />
  </Icon>
);

/** Tick / done indicator. */
export const IconCheck = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5 12l5 5 9-11" />
  </Icon>
);

/** Solid dot used for inline status / unread markers. */
export const IconDot = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="3" fill="currentColor" stroke="none" />
  </Icon>
);

/** Filled play triangle (rendered as a polygon, not an outline). */
export const IconPlay = (p: IconProps) => (
  <Icon {...p}>
    <polygon points="7 4 20 12 7 20 7 4" fill="currentColor" stroke="none" />
  </Icon>
);

/** Clock face with hands, used for "queued" / "waiting" states. */
export const IconClock = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 7v5l3 2" />
  </Icon>
);

/** Sparkle icon used to mark AI / agent affordances. */
export const IconSpark = (p: IconProps) => (
  <Icon {...p}>
    <path d="M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5L18 18M6 18l2.5-2.5M15.5 8.5L18 6" />
  </Icon>
);

/** Right-pointing arrow used for "open details" affordances. */
export const IconArrow = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5 12h14M13 6l6 6-6 6" />
  </Icon>
);

/** "@" symbol used as the "Add context" affordance in the composer. */
export const IconAt = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="4" />
    <path d="M16 12v2a3 3 0 0 0 6 0v-2a10 10 0 1 0-4 8" />
  </Icon>
);

/** Document/file glyph for file references. */
export const IconFile = (p: IconProps) => (
  <Icon {...p}>
    <path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6" />
  </Icon>
);

/** Chevron pointing right; rotated via CSS to indicate disclosure state. */
export const IconChev = (p: IconProps) => (
  <Icon {...p}>
    <path d="M9 6l6 6-6 6" />
  </Icon>
);

/** Three horizontal dots used as the overflow menu trigger. */
export const IconMore = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="5" cy="12" r="1.2" fill="currentColor" />
    <circle cx="12" cy="12" r="1.2" fill="currentColor" />
    <circle cx="19" cy="12" r="1.2" fill="currentColor" />
  </Icon>
);

/** Stacked papers icon used for copy actions. */
export const IconCopy = (p: IconProps) => (
  <Icon {...p}>
    <rect x="9" y="9" width="12" height="12" rx="2" />
    <path d="M5 15V5a2 2 0 0 1 2-2h10" />
  </Icon>
);

/** "X" close glyph for dismissing panels and dialogs. */
export const IconX = (p: IconProps) => (
  <Icon {...p}>
    <path d="M6 6l12 12M18 6L6 18" />
  </Icon>
);

/** Git fork glyph (three-node diagram) used in workflow tabs. */
export const IconGit = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="6" cy="6" r="2" />
    <circle cx="6" cy="18" r="2" />
    <circle cx="18" cy="12" r="2" />
    <path d="M6 8v8M8 6h5a3 3 0 0 1 3 3v1" />
  </Icon>
);

/** Shield icon for security / validation phases. */
export const IconShield = (p: IconProps) => (
  <Icon {...p}>
    <path d="M12 3l8 3v6c0 5-3.5 8.5-8 9-4.5-.5-8-4-8-9V6z" />
  </Icon>
);

/** Flask / lab glyph used to mark the test plan. */
export const IconFlask = (p: IconProps) => (
  <Icon {...p}>
    <path d="M9 3h6M10 3v5L4 20a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1l-6-12V3" />
  </Icon>
);

/** Paper-airplane send icon used in the chat composer. */
export const IconSend = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 12l16-8-6 18-3-8z" />
  </Icon>
);

/** Open-book glyph reserved for documentation entry points. */
export const IconBook = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 4h9a3 3 0 0 1 3 3v13H7a3 3 0 0 1-3-3zM20 4h-4a3 3 0 0 0-3 3v13h4a3 3 0 0 0 3-3z" />
  </Icon>
);

/** Color-palette icon (currently unused but reserved for the theme picker). */
export const IconPaint = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="9" />
    <circle cx="7.5" cy="10" r="1" fill="currentColor" />
    <circle cx="11" cy="7" r="1" fill="currentColor" />
    <circle cx="15.5" cy="8.5" r="1" fill="currentColor" />
    <path d="M12 21c-1 0-2-1-2-2s1-1 1-2-1-1-1-2 2-2 3-2h5" />
  </Icon>
);

/** Two opposing arrows representing a diff / compare action. */
export const IconDiff = (p: IconProps) => (
  <Icon {...p}>
    <path d="M8 3v12M4 7l4-4 4 4" />
    <path d="M16 21V9M20 17l-4 4-4-4" />
  </Icon>
);

/** Terminal frame icon used for the embedded shell tab. */
export const IconTerm = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3" y="5" width="18" height="14" rx="2" />
    <path d="M7 10l3 2-3 2M13 14h4" />
  </Icon>
);

/** Wrench icon used for tool / agent panels. */
export const IconTool = (p: IconProps) => (
  <Icon {...p}>
    <path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18v3h3l6.3-6.3a4 4 0 0 0 5.4-5.4l-2.5 2.5-2.5-2.5 2.5-2.5z" />
  </Icon>
);

/** Atom-like glyph used for the token usage chip in the workflow header. */
export const IconTokens = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2" />
  </Icon>
);
