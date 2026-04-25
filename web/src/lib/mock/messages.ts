import type { ChatMessage } from "./types";

export const MESSAGES: ChatMessage[] = [
  {
    id: "m1",
    role: "user",
    time: "10:42",
    body:
      "Let's add optimistic updates to useMutation in src/lib/query.ts. The current mutate() waits for the server — I want rollback on failure and automatic reconciliation when the real response lands.",
  },
  {
    id: "m2",
    role: "assistant",
    time: "10:42",
    body:
      "I read src/lib/query.ts, src/hooks/useMutation.ts, and the test in __tests__/useMutation.test.ts. Here's what I'll do:\n\n— Snapshot cache state before the mutation fires\n— Apply the optimistic patch synchronously so subscribers see it immediately\n— On success, replace the optimistic entry with the server response\n— On failure, roll back to the snapshot and surface the error via onError\n\nI'll thread a new optimistic field through MutationOptions<T> so existing callers keep working. Draft plan ready on the right — confirm the intent or revise.",
  },
  {
    id: "m3",
    role: "user",
    time: "10:44",
    body:
      "Looks right. One thing — the rollback has to preserve ordering for any other mutations that landed between snapshot and rollback. Don't clobber them.",
  },
  {
    id: "m4",
    role: "assistant",
    time: "10:44",
    body:
      "Good catch. I'll keep a per-key revision counter and only revert entries whose revision still matches the snapshot. Added that to the execution plan as a checkpoint between step 2 and 3.",
  },
  {
    id: "m5",
    role: "assistant",
    time: "10:46",
    streaming: true,
    body:
      "Executing Phase 2 — wiring the optimistic patch into the cache layer now. About to run the test DAG once the execution DAG settles.",
  },
];
