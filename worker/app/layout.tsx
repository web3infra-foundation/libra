import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Libra · publish",
  description:
    "Read-only Libra repository publish — code, refs and AI object model served by Cloudflare Workers.",
  robots: { index: false, follow: false }, // private-by-default; publish.md spec
};

export default function RootLayout({
  children,
}: {
  readonly children: React.ReactNode;
}) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <link rel="preconnect" href="https://fonts.googleapis.com" />
        <link rel="preconnect" href="https://fonts.gstatic.com" crossOrigin="anonymous" />
        <link
          href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&family=Source+Serif+4:ital,wght@0,400;0,500;1,500&display=swap"
          rel="stylesheet"
        />
      </head>
      <body>{children}</body>
    </html>
  );
}
