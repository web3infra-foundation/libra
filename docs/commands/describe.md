# `libra describe`

Find the nearest reachable tag for a commit and format it as a human-readable
version description.

**Alias:** `desc`

## Synopsis

```
libra describe [OPTIONS] [COMMIT]
```

## Description

`libra describe` walks the commit ancestry graph (BFS) from the given commit
(default `HEAD`) to find the closest tag. The output follows Git's describe
format:

- Exact match: `v1.2.3`
- Reachable tag with distance: `v1.2.3-4-gabc1234`
- Fallback (`--always`): `abc1234`

By default only annotated tags are considered. Pass `--tags` to also match
lightweight tags. When multiple tags are reachable at the same distance,
annotated tags are preferred; ties are broken lexicographically.

When no tag can be found and `--always` is absent, the command fails with an
actionable hint suggesting `--tags` or `--always`.

`--exact-match` restricts the command to tags that point directly at the target
commit. If no exact tag exists, it fails even when `--always` is also present.

`--dirty[=<mark>]` appends a suffix when tracked content differs from `HEAD`.
The default suffix is `-dirty`; custom marks are accepted with
`--dirty=<mark>`. Untracked files are ignored, matching Git's dirty check for
this command.

## Options

| Flag | Description | Default |
|------|-------------|---------|
| `<COMMIT>` | The commit-ish to describe. Accepts `HEAD`, branch names, tag names, raw SHA-1, `HEAD~N`. | `HEAD` |
| `--tags` | Include lightweight tags in the search (not just annotated tags). | Off |
| `--abbrev <N>` | Number of hex digits for the abbreviated commit hash in the output. | `7` |
| `--always` | When no tag can describe the target, fall back to the abbreviated commit hash instead of failing. | Off |
| `--exact-match` | Only succeed when the target commit exactly matches a tag. | Off |
| `--dirty[=<mark>]` | Append a dirty mark when tracked content differs from `HEAD`. | Off, default mark `-dirty` when enabled |

### Examples

```bash
# Describe HEAD using annotated tags only
libra describe

# Include lightweight tags
libra describe --tags

# Always produce output, even without tags
libra describe --always

# Only succeed on an exact tag match
libra describe --exact-match

# Describe a specific commit
libra describe HEAD~5

# Use longer abbreviated hashes
libra describe --abbrev 12

# Append -dirty when tracked content differs from HEAD
libra describe --dirty

# Use a custom dirty mark
libra describe --dirty=-worktree

# JSON output for automation
libra describe --json
```

## Common Commands

```bash
libra describe
libra describe --tags
libra describe --always
libra describe --exact-match
libra describe --dirty
libra describe HEAD~1
libra describe --json
libra describe --tags --abbrev 10
```

## Human Output

- Exact tag match: `v1.2.3`
- Reachable tag: `v1.2.3-4-gabc1234`
- `--always` fallback: `abc1234`
- `--dirty` on tracked changes: `v1.2.3-dirty`
- `--dirty=-worktree` on tracked changes: `v1.2.3-worktree`

`--quiet` suppresses `stdout`.

## Structured Output (JSON examples)

`--json` / `--machine` returns:

### Tag match (exact)

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "v1.2.3",
    "tag": "v1.2.3",
    "distance": 0,
    "abbreviated_commit": null,
    "exact_match": true,
    "used_always": false,
    "dirty": false,
    "dirty_mark": null
  }
}
```

### Tag match (with distance)

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "v1.2.3-4-gabc1234",
    "tag": "v1.2.3",
    "distance": 4,
    "abbreviated_commit": "abc1234",
    "exact_match": false,
    "used_always": false,
    "dirty": false,
    "dirty_mark": null
  }
}
```

### Fallback (`--always`, no tag found)

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "abc1234",
    "tag": null,
    "distance": null,
    "abbreviated_commit": "abc1234",
    "exact_match": false,
    "used_always": true,
    "dirty": false,
    "dirty_mark": null
  }
}
```

When `--always` is used and no tag matches, `tag` and `distance` are `null` and
`abbreviated_commit` contains the emitted hash.

### Dirty suffix

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "v1.2.3-dirty",
    "tag": "v1.2.3",
    "distance": 0,
    "abbreviated_commit": null,
    "exact_match": true,
    "used_always": false,
    "dirty": true,
    "dirty_mark": "-dirty"
  }
}
```

## Design Rationale

### Why no `--long`, `--match`, `--exclude`, or `--first-parent`?

Git's `describe` has accumulated many options over the years: `--long` forces
the long format even on exact matches, `--match` and `--exclude` filter tag
names by glob, `--candidates` controls how many tags to consider, and
`--first-parent` restricts the traversal. Libra deliberately ships the common
read path first: identifying a build version, requiring an exact tag when a
release script needs one, and marking tracked dirty state. The BFS-based
algorithm is straightforward and predictable. Additional flags can be added
incrementally if real users or agents need them, but starting small avoids the
combinatorial complexity that makes Git's `describe` behavior hard to reason
about (e.g., the interaction between `--match`, `--exclude`, and
`--candidates`).

### Why simplified output format?

Libra always produces the standard `tag-N-gHASH` format (or just the tag name
for exact matches). There is no `--long` flag to force the long format on
exact matches. The JSON output already includes separate `tag`, `distance`,
`abbreviated_commit`, and `exact_match` fields, so any consumer that needs to
distinguish exact-match from non-exact can check `exact_match` directly. This
is strictly more informative than Git's `--long` flag, which merely changes
the string format.

### Why BFS instead of Git's candidate algorithm?

Git's `describe` uses a more complex algorithm that considers multiple tag
candidates and picks the one with the smallest distance, with heuristics to
avoid walking the entire graph. Libra uses a simpler BFS from the target
commit, which guarantees finding the closest tag (shortest path in the DAG).
For the repository sizes Libra targets (monorepos with structured tagging),
BFS is fast enough and its behavior is trivially predictable. The trade-off
is that very deep histories with many tags could be slower than Git's pruned
search, but this has not been a problem in practice.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Default target | `HEAD` | `HEAD` | N/A (no built-in describe) |
| Annotated tags only | Default behavior | Default behavior | N/A |
| Include lightweight tags | `--tags` | `--tags` | N/A |
| Abbreviated hash length | `--abbrev <N>` (default 7) | `--abbrev=<N>` (default dynamically chosen) | N/A |
| Fallback to hash | `--always` | `--always` | N/A |
| Exact match only | `--exact-match` | `--exact-match` | N/A |
| Force long format | Not implemented (use JSON `exact_match`) | `--long` | N/A |
| Match tag pattern | Not implemented | `--match <glob>` | N/A |
| Exclude tag pattern | Not implemented | `--exclude <glob>` | N/A |
| Candidate count | All tags (BFS) | `--candidates=<N>` (default 10) | N/A |
| First-parent only | Not implemented | `--first-parent` | N/A |
| Dirty suffix | `--dirty[=<mark>]` | `--dirty[=<mark>]` | N/A |
| JSON output | `--json` with typed fields | No | No |
| Algorithm | BFS (shortest path) | Heuristic multi-candidate | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid revision | `LBR-CLI-003` | 129 |
| `HEAD` has no commit | `LBR-REPO-003` | 128 |
| No tags can describe the target and `--always` is absent | `LBR-REPO-003` | 128 |
| `--exact-match` target has no exact tag | `LBR-REPO-003` | 128 |
| Failed to read refs or objects | `LBR-IO-001` / `LBR-REPO-002` | 128 |
