/**
 * Barrel export for the mock data fixtures and their shared types.
 *
 * Centralising these exports lets components import from `@/lib/mock` without
 * caring about file structure, and gives a single place to swap a fixture for
 * a live data source later.
 */
export * from "./types";
export { PHASES } from "./phases";
export { THREADS } from "./threads";
export { MESSAGES } from "./messages";
export { WORKFLOW } from "./workflow";
export { SUMMARY } from "./summary";
export { REVIEW } from "./review";
export { TERMINAL_LINES } from "./terminal";
