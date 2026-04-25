import type { SummaryState } from "./types";

export const SUMMARY: SummaryState = {
  progress: [
    { done: true, text: "Read src/lib/query.ts and snapshot current cache shape" },
    { done: true, text: "Design MutationOptions<T> extension with optional optimistic field" },
    { done: true, text: "Implement cache.snapshot() + cache.rollback() primitives" },
    { done: false, text: "Wire per-key revision counter so rollback preserves ordering" },
    { done: false, text: "Cover happy + error path with tests in __tests__/useMutation.test.ts" },
  ],
  branch: {
    name: "agent/optimistic-mutate",
    base: "main",
    pr: "No pull request",
    changes: "2 files changed, 1 untracked",
  },
  artifacts: [
    { kind: "PatchSet", id: "ps-07", meta: "+46 −7 across 2 files" },
    { kind: "Frame", id: "cf-0418", meta: "cache shape captured" },
  ],
  todo: [
    { done: true, text: "Snapshot cache at mutate() entry" },
    { done: true, text: "Apply optimistic patch to subscribers" },
    { done: false, text: "Per-key revision counter for safe rollback" },
    { done: false, text: "Reconcile server response into cache" },
    { done: false, text: "Surface onError with rollback context" },
    { done: false, text: "Update MutationOptions<T> JSDoc" },
  ],
};
