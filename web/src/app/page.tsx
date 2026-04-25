import type { Metadata } from "next";

import { Workspace } from "@/components/workspace/workspace";

export const metadata: Metadata = {
  title: "Libra — Agent Workspace",
};

export default function Home() {
  return <Workspace />;
}
