import type { Metadata } from "next";
import localFont from "next/font/local";

import { cn } from "@/lib/utils";
import "./globals.css";

const inter = localFont({
  src: "./fonts/inter-latin-variable.woff2",
  variable: "--font-inter",
  weight: "100 900",
  display: "swap",
  fallback: ["Arial", "system-ui", "sans-serif"],
});

const jetBrainsMono = localFont({
  src: "./fonts/jetbrains-mono-latin-variable.woff2",
  variable: "--font-jetbrains-mono",
  weight: "100 800",
  display: "swap",
  fallback: ["ui-monospace", "SFMono-Regular", "Menlo", "monospace"],
});

export const metadata: Metadata = {
  title: {
    default: "Libra — Agent Workspace",
    template: "%s | Libra",
  },
  description:
    "Libra agent workspace — a five-phase pipeline for AI-driven, intent-anchored code change.",
};

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
