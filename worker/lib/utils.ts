import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: readonly ClassValue[]): string {
  return twMerge(clsx(inputs));
}

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "0 B";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toISOString().replace("T", " ").replace(/\.\d+Z$/, "Z");
}

export function shortRevision(oid: string | null | undefined): string {
  if (!oid) return "—";
  return oid.length <= 12 ? oid : oid.slice(0, 12);
}

/**
 * Encode a repository-relative path for use in a URL path component.
 *
 * Codex pass-1 P2: D1 paths can contain characters that are valid in
 * a tree but URL-active when interpolated (`?`, `#`, `%`, spaces, …).
 * Encoding each segment individually while preserving the `/`
 * separator avoids both ambiguity and double-encoding of slashes.
 */
export function encodePathForUrl(path: string): string {
  if (path === "") return "";
  return path.split("/").map((segment) => encodeURIComponent(segment)).join("/");
}

/**
 * Page-side path validator. Mirrors the API `parsePath` rules without
 * pulling `lib/server/*` into the React Server Component bundle.
 *
 * Codex pass-1 P2 + pass-11 P3: the page validator must match the
 * API validator. An earlier draft rejected ANY path containing
 * `..` substring, which incorrectly rejected legal filenames like
 * `foo..bar.txt`. The API validator splits on `/` and rejects only
 * the exact `..` segment; this helper does the same.
 *
 * Returns `true` when the path is safe; `false` to fall through to
 * the page's `notFound()` UI. `""` (repo root) is allowed because
 * the catch-all routes also match the empty trailing segment.
 */
export function isPagePathSafe(path: string): boolean {
  if (path.length > 4096) return false;
  if (path === "") return true;
  if (path.includes("\0")) return false;
  if (path.startsWith("/") || path.includes("//")) return false;
  for (const segment of path.split("/")) {
    if (segment === ".." || segment.length === 0) return false;
  }
  return true;
}
