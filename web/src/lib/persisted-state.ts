/**
 * `useState`-shaped hooks that mirror their value into `localStorage` and
 * keep all subscribers across the page in sync.
 *
 * The hooks rely on `useSyncExternalStore` so React produces tear-free reads
 * during concurrent rendering. A small in-memory pub/sub registry keyed by
 * the storage key fans state changes out to every subscriber that reads the
 * same key — for example, opening the terminal in one panel updates the
 * state in any other component reading `libra.termOpen`.
 *
 * SSR contract: the third argument to `useSyncExternalStore` returns the
 * fallback value, so the hooks render deterministically on the server (no
 * hydration mismatch) and pick up the persisted value on the first client
 * commit.
 */
"use client";

import { useCallback, useSyncExternalStore } from "react";

import { readBoolean, readNumber, writeBoolean, writeNumber } from "./storage";

/** DOM event name reserved for cross-tab broadcasts; currently unused but exported for future use. */
const EVENT_NAME = "libra:persisted-state";

type Listener = () => void;

// Listener registry: one set of callbacks per storage key. Using a Map keeps
// listener buckets isolated so emitting on key A only wakes up subscribers
// for key A.
const listeners = new Map<string, Set<Listener>>();

/**
 * Register a listener for changes to `key`. Returns an unsubscribe function
 * suitable for `useSyncExternalStore`. Empty buckets are not pruned because
 * the cost of leaking a Map entry is negligible relative to repeated insert.
 */
function subscribe(key: string, listener: Listener) {
  let bucket = listeners.get(key);
  if (!bucket) {
    bucket = new Set();
    listeners.set(key, bucket);
  }
  bucket.add(listener);
  return () => {
    bucket?.delete(listener);
  };
}

/** Notify every listener bound to `key` that its underlying value changed. */
function emit(key: string) {
  const bucket = listeners.get(key);
  if (!bucket) return;
  bucket.forEach((l) => l());
}

/**
 * `useState<number>` that persists to `localStorage`.
 *
 * Returns a tuple of `[value, setValue]`. `setValue` accepts either a value
 * or an updater function (mirrors `React.useState`); the updater receives
 * the latest persisted value (read fresh from storage), not a closure-captured
 * snapshot.
 *
 * SSR: the server snapshot always returns `fallback`, so the first server
 * render is stable. After hydration React re-runs the client snapshot and
 * the persisted value flows in.
 *
 * @param key - localStorage key. Conventionally prefixed with `libra.`.
 * @param fallback - Default value when nothing is stored or storage fails.
 */
export function useStoredNumber(
  key: string,
  fallback: number,
): readonly [number, (next: number | ((prev: number) => number)) => void] {
  const value = useSyncExternalStore(
    useCallback((cb) => subscribe(key, cb), [key]),
    useCallback(() => readNumber(key, fallback), [key, fallback]),
    useCallback(() => fallback, [fallback]),
  );

  const setValue = useCallback(
    (next: number | ((prev: number) => number)) => {
      // Updater functions read fresh storage so concurrent setters compose
      // correctly even if React hasn't yet flushed the latest render.
      const resolved =
        typeof next === "function"
          ? (next as (prev: number) => number)(readNumber(key, fallback))
          : next;
      writeNumber(key, resolved);
      emit(key);
    },
    [key, fallback],
  );

  return [value, setValue] as const;
}

/**
 * `useState<boolean>` variant of {@link useStoredNumber}. Same SSR
 * contract and updater semantics.
 *
 * @param key - localStorage key.
 * @param fallback - Default boolean when nothing is stored.
 */
export function useStoredBoolean(
  key: string,
  fallback: boolean,
): readonly [boolean, (next: boolean | ((prev: boolean) => boolean)) => void] {
  const value = useSyncExternalStore(
    useCallback((cb) => subscribe(key, cb), [key]),
    useCallback(() => readBoolean(key, fallback), [key, fallback]),
    useCallback(() => fallback, [fallback]),
  );

  const setValue = useCallback(
    (next: boolean | ((prev: boolean) => boolean)) => {
      const resolved =
        typeof next === "function"
          ? (next as (prev: boolean) => boolean)(readBoolean(key, fallback))
          : next;
      writeBoolean(key, resolved);
      emit(key);
    },
    [key, fallback],
  );

  return [value, setValue] as const;
}

export { EVENT_NAME };
