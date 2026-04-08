import type { Metadata } from "next";

import { CodeSessionPage } from "@/components/code-session/code-session-page";

export const metadata: Metadata = {
  title: "Libra Code",
};

export default function Home() {
  return <CodeSessionPage />;
}
