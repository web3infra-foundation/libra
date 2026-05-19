/**
 * Tiny unified-diff parser used by the Review tab.
 *
 * Handles only the subset of unified diff that `git diff --no-color` emits:
 *   - Hunk headers `@@ -A,B +C,D @@ context`.
 *   - `+`, `-`, ` ` lines.
 *   - Anything else (binary marker, file headers, "\ No newline" footer)
 *     gets surfaced via `parseError` and `rawDiff` so the panel can fall
 *     back to a plain-text view rather than throwing.
 */

export type DiffLineKind = "ctx" | "add" | "del";

export type DiffLine = {
  kind: DiffLineKind;
  n1?: number;
  n2?: number;
  text: string;
};

export type DiffHunk = {
  header: string;
  lines: DiffLine[];
};

export type DiffFile = {
  path: string;
  changeType: string;
  add: number;
  del: number;
  hunks: DiffHunk[];
  /** Original diff text (preserved for the fallback view when parsing fails). */
  rawDiff: string | null;
  /** Filled when the parser couldn't make sense of the diff body. */
  parseError?: string;
};

const HUNK_HEADER_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/;

export function parseUnifiedDiff(
  path: string,
  diff: string | null,
  changeType: string,
): DiffFile {
  if (!diff || diff.trim().length === 0) {
    return { path, changeType, add: 0, del: 0, hunks: [], rawDiff: diff };
  }
  const lines = diff.split(/\r?\n/);
  const hunks: DiffHunk[] = [];
  let current: DiffHunk | null = null;
  let n1 = 0;
  let n2 = 0;
  let add = 0;
  let del = 0;
  let parseError: string | undefined;

  for (const line of lines) {
    if (line.startsWith("@@")) {
      const match = HUNK_HEADER_RE.exec(line);
      if (!match) {
        parseError = `malformed hunk header: ${line}`;
        break;
      }
      n1 = Number.parseInt(match[1], 10);
      n2 = Number.parseInt(match[2], 10);
      current = { header: line, lines: [] };
      hunks.push(current);
      continue;
    }
    if (!current) {
      // Skip pre-hunk file headers ("diff --git", "index", "+++", "---").
      if (
        line.startsWith("diff ") ||
        line.startsWith("index ") ||
        line.startsWith("--- ") ||
        line.startsWith("+++ ") ||
        line.length === 0
      ) {
        continue;
      }
      parseError = `unexpected line before any hunk: ${line.slice(0, 40)}`;
      break;
    }
    if (line.startsWith("+")) {
      current.lines.push({ kind: "add", n2, text: line.slice(1) });
      n2 += 1;
      add += 1;
    } else if (line.startsWith("-")) {
      current.lines.push({ kind: "del", n1, text: line.slice(1) });
      n1 += 1;
      del += 1;
    } else if (line.startsWith(" ")) {
      current.lines.push({ kind: "ctx", n1, n2, text: line.slice(1) });
      n1 += 1;
      n2 += 1;
    } else if (line.startsWith("\\")) {
      // "\ No newline at end of file" — ignore.
    } else if (line.length === 0) {
      current.lines.push({ kind: "ctx", n1, n2, text: "" });
    } else {
      parseError = `unrecognised diff line: ${line.slice(0, 40)}`;
      break;
    }
  }

  return {
    path,
    changeType,
    add,
    del,
    hunks: parseError ? [] : hunks,
    rawDiff: parseError ? diff : null,
    parseError,
  };
}
