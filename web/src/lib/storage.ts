"use client";

export function readNumber(key: string, fallback: number): number {
  if (typeof window === "undefined") return fallback;
  try {
    const v = parseInt(window.localStorage.getItem(key) ?? "", 10);
    return Number.isFinite(v) ? v : fallback;
  } catch {
    return fallback;
  }
}

export function writeNumber(key: string, v: number): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, String(v));
  } catch {
    // ignore
  }
}

export function readBoolean(key: string, fallback: boolean): boolean {
  if (typeof window === "undefined") return fallback;
  try {
    const v = window.localStorage.getItem(key);
    return v === null ? fallback : v === "true";
  } catch {
    return fallback;
  }
}

export function writeBoolean(key: string, v: boolean): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, String(v));
  } catch {
    // ignore
  }
}

export function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}
