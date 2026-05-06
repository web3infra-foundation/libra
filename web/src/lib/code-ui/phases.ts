/**
 * Static descriptors for the five Libra agent pipeline phases.
 *
 * `n` doubles as the canonical zero-based phase index used elsewhere in the
 * UI. Lifted out of `lib/mock/phases.ts` so the live UI can keep referencing
 * the same canonical labels without depending on the mock fixture module.
 */

/** The five phases of the Libra agent pipeline. Stable, used as a discriminator. */
export type PhaseKey = "intent" | "plan" | "execution" | "validate" | "release";

/** Human-readable descriptor for a phase shown in the {@link PhaseStrip}. */
export type PhaseDescriptor = {
  /** Zero-based phase index, also used to compare against snapshot-derived phase. */
  n: number;
  key: PhaseKey;
  /** Short label like "Phase 0". */
  label: string;
  /** Title rendered under the phase chip (e.g. "Intent"). */
  name: string;
  /** One-line tagline shown beneath the title. */
  blurb: string;
};

export const PHASES: PhaseDescriptor[] = [
  { n: 0, key: "intent", label: "Phase 0", name: "Intent", blurb: "Draft & confirm" },
  { n: 1, key: "plan", label: "Phase 1", name: "Plan", blurb: "Analyze & confirm" },
  { n: 2, key: "execution", label: "Phase 2", name: "Execution", blurb: "Stage-gated DAG" },
  { n: 3, key: "validate", label: "Phase 3", name: "Validation", blurb: "Audit & evidence" },
  { n: 4, key: "release", label: "Phase 4", name: "Release", blurb: "Decision" },
];
