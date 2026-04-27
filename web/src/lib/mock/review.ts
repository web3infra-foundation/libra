/**
 * Diff review fixture.
 *
 * Hand-authored unified-diff data shown by the Diff tab. Numbers in `n1`/`n2`
 * are old/new line numbers respectively; lines without `n1` are pure
 * additions and lines without `n2` are pure deletions.
 */
import type { ReviewState } from "./types";

export const REVIEW: ReviewState = {
  stats: { files: 2, add: 46, del: 7 },
  files: [
    {
      path: "src/lib/query.ts",
      add: 34,
      del: 7,
      hunks: [
        {
          header: "@@ -214,10 +214,23 @@ export function useMutation<T>(",
          lines: [
            { kind: "ctx", n1: 214, n2: 214, text: "    const [state, setState] = React.useState<State<T>>({ idle: true });" },
            { kind: "ctx", n1: 215, n2: 215, text: "" },
            { kind: "ctx", n1: 216, n2: 216, text: "    async function mutate(input: TInput) {" },
            { kind: "del", n1: 217, text: "      const result = await fetcher(input);" },
            { kind: "del", n1: 218, text: "      cache.set(key, result);" },
            { kind: "del", n1: 219, text: "      setState({ idle: false, data: result });" },
            { kind: "add", n2: 217, text: "      const snap = cache.snapshot(key);" },
            { kind: "add", n2: 218, text: "      if (options.optimistic) {" },
            { kind: "add", n2: 219, text: "        cache.patch(key, options.optimistic(input));" },
            { kind: "add", n2: 220, text: "      }" },
            { kind: "add", n2: 221, text: "      try {" },
            { kind: "add", n2: 222, text: "        const result = await fetcher(input);" },
            { kind: "add", n2: 223, text: "        cache.reconcile(key, snap.rev, result);" },
            { kind: "add", n2: 224, text: "        setState({ idle: false, data: result });" },
            { kind: "add", n2: 225, text: "      } catch (err) {" },
            { kind: "add", n2: 226, text: "        cache.rollback(key, snap);" },
            { kind: "add", n2: 227, text: "        options.onError?.(err, { rolledBack: true });" },
            { kind: "add", n2: 228, text: "        throw err;" },
            { kind: "add", n2: 229, text: "      }" },
            { kind: "ctx", n1: 220, n2: 230, text: "    }" },
            { kind: "ctx", n1: 221, n2: 231, text: "" },
            { kind: "ctx", n1: 222, n2: 232, text: "    return { state, mutate };" },
          ],
        },
      ],
    },
    {
      path: "src/lib/cache.ts",
      add: 12,
      del: 0,
      hunks: [
        {
          header: "@@ -88,3 +88,15 @@ export class Cache {",
          lines: [
            { kind: "ctx", n1: 88, n2: 88, text: "  set(key: Key, value: Value) {" },
            { kind: "ctx", n1: 89, n2: 89, text: "    this.store.set(key, { value, rev: ++this.rev });" },
            { kind: "ctx", n1: 90, n2: 90, text: "  }" },
            { kind: "add", n2: 91, text: "" },
            { kind: "add", n2: 92, text: "  snapshot(key: Key): Snap {" },
            { kind: "add", n2: 93, text: "    const entry = this.store.get(key);" },
            { kind: "add", n2: 94, text: "    return { key, rev: entry?.rev ?? 0, value: entry?.value };" },
            { kind: "add", n2: 95, text: "  }" },
            { kind: "add", n2: 96, text: "" },
            { kind: "add", n2: 97, text: "  rollback(key: Key, snap: Snap) {" },
            { kind: "add", n2: 98, text: "    const current = this.store.get(key);" },
            { kind: "add", n2: 99, text: "    if (current && current.rev !== snap.rev + 1) return;" },
            { kind: "add", n2: 100, text: "    this.store.set(key, { value: snap.value, rev: ++this.rev });" },
            { kind: "add", n2: 101, text: "  }" },
          ],
        },
      ],
    },
  ],
};
