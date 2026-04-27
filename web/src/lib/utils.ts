/**
 * Tiny utility helpers shared across the UI.
 *
 * Intentionally framework-agnostic; nothing here imports React or browser-only
 * APIs so the helpers can run during SSG/export.
 */
import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

/**
 * Compose Tailwind class lists safely.
 *
 * Runs the inputs through `clsx` (to flatten arrays/objects/conditionals)
 * and then `twMerge` (which de-duplicates conflicting Tailwind utilities,
 * keeping only the last-specified one).
 *
 * Boundary conditions: empty / `false` / `null` / `undefined` inputs are
 * silently dropped. Non-Tailwind class names pass through unchanged.
 *
 * @param inputs - Any combination of strings, arrays, or class-value objects
 *                 accepted by `clsx`.
 * @returns A single space-separated class string ready for the `className` prop.
 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
