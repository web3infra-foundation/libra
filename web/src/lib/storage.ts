/**
 * Browser `localStorage` wrappers that survive SSR and storage failures.
 *
 * Every helper guards on `typeof window === "undefined"` so they can be
 * imported (but no-op) during static export. Accesses are wrapped in
 * `try/catch` because `localStorage` can throw when disabled (private mode,
 * quota exhaustion, sandbox iframes).
 *
 * Storage shape: values are serialized as raw strings (numbers via `String`,
 * booleans as the literal text `"true"` / `"false"`). Keep keys namespaced
 * with the `libra.` prefix to avoid collisions with other apps on the same
 * origin during dev.
 */
"use client";

/**
 * Read a finite integer from localStorage.
 *
 * Returns `fallback` if: window is undefined (SSR), the key is missing,
 * the stored value is not a valid base-10 integer, or `localStorage` access
 * throws.
 *
 * @param key - localStorage key.
 * @param fallback - Value returned when no usable number is stored.
 */
export function readNumber(key: string, fallback: number): number {
  if (typeof window === "undefined") return fallback;
  try {
    const v = parseInt(window.localStorage.getItem(key) ?? "", 10);
    return Number.isFinite(v) ? v : fallback;
  } catch {
    return fallback;
  }
}

/**
 * Persist a number under `key`. Silently no-ops on SSR or when storage is
 * unavailable so callers do not need to handle thrown errors.
 *
 * @param key - localStorage key.
 * @param v - Number to serialise via `String(v)`.
 */
export function writeNumber(key: string, v: number): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, String(v));
  } catch {
    // ignore
  }
}

/**
 * Read a boolean from localStorage.
 *
 * Boundary: any value other than the literal string `"true"` (including
 * `"false"`, the empty string, or junk) returns `false`. A missing key
 * returns `fallback`.
 *
 * @param key - localStorage key.
 * @param fallback - Value returned when the key is missing or storage fails.
 */
export function readBoolean(key: string, fallback: boolean): boolean {
  if (typeof window === "undefined") return fallback;
  try {
    const v = window.localStorage.getItem(key);
    return v === null ? fallback : v === "true";
  } catch {
    return fallback;
  }
}

/**
 * Persist a boolean as the literal string `"true"` or `"false"`. Mirrors
 * {@link writeNumber} in error tolerance.
 *
 * @param key - localStorage key.
 * @param v - Boolean to persist.
 */
export function writeBoolean(key: string, v: boolean): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, String(v));
  } catch {
    // ignore
  }
}

/**
 * Clamp a number to the closed interval `[lo, hi]`.
 *
 * Boundary: if `lo > hi` the function returns `hi` (the inner `min` wins).
 * `NaN` propagates through unchanged.
 *
 * @param v - Input number.
 * @param lo - Lower bound (inclusive).
 * @param hi - Upper bound (inclusive).
 */
export function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}
