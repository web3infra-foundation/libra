"use client";

import { useCallback, useSyncExternalStore } from "react";

import { readBoolean, readNumber, writeBoolean, writeNumber } from "./storage";

const EVENT_NAME = "libra:persisted-state";

type Listener = () => void;

const listeners = new Map<string, Set<Listener>>();

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

function emit(key: string) {
  const bucket = listeners.get(key);
  if (!bucket) return;
  bucket.forEach((l) => l());
}

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
