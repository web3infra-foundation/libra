/**
 * Sidebar threads fixture.
 *
 * The thread with `active: true` is selected on mount; `phase` indexes into
 * {@link PHASES} to render the phase chip on each row.
 */
import type { Thread } from "./types";

export const THREADS: Thread[] = [
  { id: "t1", title: "Add optimistic updates to useMutation", ago: "1m", active: true, phase: 2 },
  { id: "t2", title: "Refactor auth provider to context v3", ago: "18m", phase: 4 },
  { id: "t3", title: "Migrate Table to virtualized rows", ago: "1h", phase: 2 },
  { id: "t4", title: "Fix SSR hydration warning in <Toast>", ago: "3h", phase: 4 },
  { id: "t5", title: "Audit zod schemas for form module", ago: "1d", phase: 3 },
  { id: "t6", title: "Dark mode tokens for Settings panel", ago: "2d", phase: 4 },
  { id: "t7", title: "Type-narrow router params helper", ago: "4d", phase: 0 },
];
