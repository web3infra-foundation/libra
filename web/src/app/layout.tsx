/**
 * Root layout for the Libra web app.
 *
 * Wraps every page in `<html>` / `<body>` and registers two locally hosted
 * variable fonts (Inter for sans-serif, JetBrains Mono for monospace). Fonts
 * are bundled rather than fetched from a CDN so the static export works in
 * fully air-gapped sandboxes used by `libra code`.
 *
 * Also supplies the default `<head>` metadata (title + description) for SEO
 * and browser tab labels.
 */
import type { Metadata } from "next";
import localFont from "next/font/local";

import { cn } from "@/lib/utils";
import "./globals.css";

// Inter variable font — exposed as `--font-inter` for the Tailwind sans family.
// `display: "swap"` avoids FOIT; fallbacks keep the layout stable before the
// font finishes downloading.
const inter = localFont({
  src: "./fonts/inter-latin-variable.woff2",
  variable: "--font-inter",
  weight: "100 900",
  display: "swap",
  fallback: ["Arial", "system-ui", "sans-serif"],
});

// JetBrains Mono variable font — used by the `.mono` utility class for terminal
// output, code spans, hashes, and any tabular numeric content.
const jetBrainsMono = localFont({
  src: "./fonts/jetbrains-mono-latin-variable.woff2",
  variable: "--font-jetbrains-mono",
  weight: "100 800",
  display: "swap",
  fallback: ["ui-monospace", "SFMono-Regular", "Menlo", "monospace"],
});

/** Top-level page metadata; the template applies to nested pages that override `title`. */
export const metadata: Metadata = {
  title: {
    default: "Libra — Agent Workspace",
    template: "%s | Libra",
  },
  description:
    "Libra agent workspace — a five-phase pipeline for AI-driven, intent-anchored code change.",
};

/**
 * Root layout component.
 *
 * Renders the html shell with both font CSS variables enabled so any
 * descendant can pick up `font-sans` (Inter) or the `.mono` class
 * (JetBrains Mono) without further setup.
 *
 * @param children - The current route's rendered tree.
 */
export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      className={cn("font-sans", inter.variable, jetBrainsMono.variable)}
    >
      <body>{children}</body>
    </html>
  );
}
