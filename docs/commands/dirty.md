# `libra dirty`

Advisory dirty-set marks (a Libra extension, lore.md §1.1). Git has no
equivalent; the cache accelerates status for agents and tooling.

## Synopsis

```
libra dirty <paths>...
libra dirty --list
```

## Description

`libra dirty <paths>` marks paths dirty in the `working_dirty` SQLite cache —
no file contents are read and the index is never touched. Marks are advisory
and can only make the cached view *over*-report (the safe direction).
Nonexistent paths are legal: a deletion IS dirty. Paths must stay inside the
repository (an escaping path fails the whole invocation, exit 129).

The cache lifecycle:

- **`libra status --scan`** — the only authoritative rebuild: runs the normal
  full status and atomically replaces the snapshot (unstaged dirty set + the
  staged set), stamped with the index fingerprint and HEAD.
- **`libra status --cached`** — consumes the snapshot instead of walking the
  worktree. Any freshness doubt (index or HEAD changed since the scan; no
  scan yet) degrades to the full status with a hint — the cache never lies.
  **Snapshot semantics**: worktree-only edits made after the scan don't
  change the index and are invisible to `--cached` until a rescan or a
  `libra dirty` mark records them — that is what the marks are for.
- **`libra status --check-dirty`** — re-verifies only the cached set
  (O(dirty paths)): rows re-verified clean are pruned; nothing new is found.
- **`libra dirty --list`** — shows the cached rows (`kind`, `source`, path)
  and the cache's freshness.

Default `libra status` never reads or writes the cache.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. |
| `128` | Not a repository. |
| `129` | Usage errors (escaping paths, missing arguments). |

## Examples

```bash
libra status --scan               # build the snapshot
libra dirty src/main.rs           # record an edit without a rescan
libra status --cached             # O(dirty) status from the cache
libra status --check-dirty        # prune stale marks
libra --json dirty --list         # structured cache inspection
```

## Comparison with Git

Git has no dirty-set cache surface (its closest machinery is the index stat
cache and `fsmonitor`, both internal). `libra dirty` and the `status`
`--scan`/`--cached`/`--check-dirty` flags are classified
`intentionally-different` in [`COMPATIBILITY.md`](../../COMPATIBILITY.md).
Note `status --cached` is unrelated to Git's `--cached` (= the index).
