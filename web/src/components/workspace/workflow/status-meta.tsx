import type { ReactNode } from "react";

import { IconCheck, IconX } from "@/components/icons";
import type { StepStatus } from "@/lib/mock";

export type StatusMeta = {
  icon: ReactNode;
  color: string;
  bg: string;
  label: string;
};

export function statusMeta(status: StepStatus): StatusMeta {
  switch (status) {
    case "done":
      return {
        icon: <IconCheck size={10} sw={2.5} />,
        color: "var(--good)",
        bg: "var(--good-soft)",
        label: "DONE",
      };
    case "running":
      return {
        icon: <span className="libra-spin" />,
        color: "var(--accent)",
        bg: "var(--accent-soft)",
        label: "RUNNING",
      };
    case "failed":
      return {
        icon: <IconX size={10} sw={2.5} />,
        color: "var(--bad)",
        bg: "var(--bad-soft)",
        label: "FAILED",
      };
    default:
      return {
        icon: (
          <span
            className="block h-[5px] w-[5px] rounded-full"
            style={{ background: "var(--ink-3)" }}
          />
        ),
        color: "var(--ink-3)",
        bg: "var(--paper-2)",
        label: "QUEUED",
      };
  }
}
