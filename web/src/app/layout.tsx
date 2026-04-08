import type { Metadata } from "next";
import { Inter, JetBrains_Mono } from "next/font/google";

import { cn } from "@/lib/utils";
import "./globals.css";

const inter = Inter({
  subsets: ["latin"],
  variable: "--font-inter",
  display: "swap",
});

const jetBrainsMono = JetBrains_Mono({
  subsets: ["latin"],
  variable: "--font-jetbrains-mono",
  display: "swap",
});

export const metadata: Metadata = {
  title: {
    default: "Libra",
    template: "%s | Libra",
  },
  description: "Internal tooling surfaces for monorepo change control.",
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
      <body className="antialiased">{children}</body>
    </html>
  );
}
