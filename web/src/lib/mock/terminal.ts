/**
 * Initial scrollback for the embedded terminal.
 *
 * Each entry's `kind` controls coloring and the leading marker — see
 * `terminal.tsx::lineMark` and `lineTone` for the visual mapping.
 */
import type { TerminalLine } from "./types";

export const TERMINAL_LINES: TerminalLine[] = [
  { kind: "meta", text: "libra sandbox v0.4.2 · image rust:1.81-slim · net=off · fs=rw(tmp)" },
  { kind: "meta", text: "mount: /workspace → agent/optimistic-mutate @ 7f3a9e1" },
  { kind: "prompt", text: "cargo test --lib optimistic" },
  { kind: "stdout", text: "   Compiling libra-cache v0.3.1 (/workspace)" },
  { kind: "stdout", text: "   Compiling libra-hooks v0.1.4 (/workspace)" },
  { kind: "stdout", text: "    Finished test [unoptimized + debuginfo] target(s) in 3.42s" },
  { kind: "stdout", text: "     Running unittests src/lib.rs (target/debug/deps/libra_cache-7c8f1a)" },
  { kind: "stdout", text: "" },
  { kind: "stdout", text: "running 3 tests" },
  { kind: "pass", text: "test optimistic::snapshot_before_mutate ... ok" },
  { kind: "pass", text: "test optimistic::patch_visible_synchronously ... ok" },
  { kind: "run", text: "test optimistic::rollback_preserves_concurrent ... running" },
  { kind: "stdout", text: "" },
  { kind: "info", text: "[agent] capturing PatchSet ps-07 (+46 −7 across 2 files)" },
  { kind: "info", text: "[agent] revision guard open: cache key \"users:42\" rev=4→5" },
  { kind: "warn", text: "warning: unused variable `prev_snapshot` (will be used once rollback lands)" },
];
