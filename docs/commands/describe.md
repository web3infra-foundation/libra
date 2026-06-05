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

## Options

| Flag | Description | Default |
|------|-------------|---------|
| `<COMMIT>` | The commit-ish to describe. Accepts `HEAD`, branch names, tag names, raw SHA-1, `HEAD~N`. | `HEAD` |
| `--tags` | Include lightweight tags in the search (not just annotated tags). | Off |
| `--abbrev <N>` | Number of hex digits for the abbreviated commit hash in the output. | `7` |
| `--always` | When no tag can describe the target, fall back to the abbreviated commit hash instead of failing. | Off |
| `--exact-match` | Only succeed when a tag points at the commit exactly (distance 0); otherwise fail. | Off |
| `--first-parent` | Follow only the first parent of merge commits when walking history. | Off |
| `--match <PATTERN>` | Only consider tags whose name matches the glob (repeatable; OR-combined). | — |
| `--exclude <PATTERN>` | Exclude tags whose name matches the glob (repeatable; takes precedence over `--match`). | — |
| `--dirty[=<MARK>]` | Append a marker (default `-dirty`) when the worktree has tracked changes. Untracked-only changes never count. | Off |
| `--contains` | Reverse search: print which ref's history contains the commit as `<refname>~<offset>`. Includes lightweight tags by default. | Off |
| `--candidates <N>` | Consider at most `N` candidate tags (rejects `0`; overridable via `describe.maxCandidates`). | `describe.maxCandidates` |
| `--all` | With `--contains`, also search local branch heads and remote-tracking branches (not just tags). | Off |

Glob patterns use [`wax`](https://docs.rs/wax) syntax and are capped at 256 characters; a longer
or malformed pattern is rejected as a usage error (`LBR-CLI-002`, exit 129).

### Examples

```bash
# Describe HEAD using annotated tags only
libra describe

# Include lightweight tags
libra describe --tags

# Always produce output, even without tags
libra describe --always

# Describe a specific commit
libra describe HEAD~5

# Use longer abbreviated hashes
libra describe --abbrev 12

# Only succeed at an exact tag
libra describe --exact-match

# Follow only the first parent through merges
libra describe --first-parent

# Filter tags by glob (exclude wins over match)
libra describe --match 'v1.*' --exclude '*-rc*'

# Append -dirty (or a custom marker) when the worktree has tracked changes
libra describe --dirty
libra describe --dirty=-wip

# Reverse lookup: which ref contains this commit?
libra describe --contains HEAD~3
libra describe --contains --all HEAD~3

# Bound the candidate-tag search
libra describe --candidates 5

# JSON output for automation
libra describe --json
```

## Common Commands

```bash
libra describe
libra describe --tags
libra describe --always
libra describe HEAD~1
libra describe --json
libra describe --tags --abbrev 10
libra describe --match 'v*' --exclude '*-rc*'
libra describe --dirty
libra describe --contains HEAD~1
libra describe --candidates 5
```

## Human Output

- Exact tag match: `v1.2.3`
- Reachable tag: `v1.2.3-4-gabc1234`
- `--always` fallback: `abc1234`

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
    "used_always": false
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
    "used_always": false
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
    "used_always": true
  }
}
```

When `--always` is used and no tag matches, `tag` and `distance` are `null` and
`abbreviated_commit` contains the emitted hash.

### `--contains` (reverse lookup)

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD~3",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "v1.2.3~3",
    "tag": "v1.2.3",
    "distance": null,
    "abbreviated_commit": null,
    "exact_match": false,
    "used_always": false,
    "dirty": false,
    "dirty_suffix": null,
    "contains_offset": 3,
    "ref_kind": "tag",
    "ref_name": "v1.2.3"
  }
}
```

The output object always carries these additive fields:

- `dirty` / `dirty_suffix` — set when `--dirty` detects tracked changes (the suffix
  appended to `result`).
- `contains_offset` — the `~N` offset for `--contains`.
- `ref_kind` (`"tag"` / `"head"` / `"remote"`) and `ref_name` (e.g. `heads/main`) —
  populated for `--contains` (and `--all`). For a branch/remote match `tag` is `null`.

## Design Rationale

### Match/exclude, dirty, contains, candidates are now supported

An earlier version of Libra deliberately shipped a minimal subset of `git
describe` and documented `--match`, `--exclude`, `--candidates`,
`--first-parent`, and `--dirty` as intentionally unimplemented. **That decision
has been reversed.** Version-generation scripts and release tooling broadly
depend on these flags (`vX.Y.Z-N-gHASH`, `-dirty` suffixes, `--contains`
offsets), so the gap broke interoperability. Libra now implements
`--first-parent`, `--exact-match`, `--match`, `--exclude`, `--dirty`,
`--contains`, `--candidates`, and `--all`, while keeping a few deliberate
differences (below).

### Still simplified: no `--long`

Libra always produces the standard `tag-N-gHASH` format (or just the tag name
for exact matches). There is no `--long` flag to force the long format on
exact matches. The JSON output already includes separate `tag`, `distance`,
`abbreviated_commit`, and `exact_match` fields, so any consumer that needs to
distinguish exact-match from non-exact can check `exact_match` directly — this
is strictly more informative than Git's `--long`, which merely changes the
string format.

### BFS shortest-path vs Git's candidate heuristic

Git's `describe` uses a heuristic that considers multiple tag candidates with
pruning to avoid walking the entire graph. Libra uses a bounded BFS from the
target commit, which guarantees finding the closest tag (shortest path in the
DAG). `--candidates` and `describe.maxCandidates` bound how many candidate tags
the walk collects, but because the BFS already returns the topologically
nearest tag, the *result* is deterministic regardless of `N`. The walk is
capped at 10,000 commits; on a deeper history it fails with `LBR-REPO-003`
(exit 128) unless `--always` is given.

### Fixed default `--abbrev` of 7

Libra's `--abbrev` defaults to a fixed 7 digits; Git chooses the shortest
unique length dynamically. Scripts that need determinism should pass an
explicit `--abbrev=<N>`.

### `--all` scope (partial)

`--all` extends the candidate ref set for `--contains` to local branch heads
and remote-tracking branches (in addition to tags). It does not enumerate
`refs/notes` or `refs/stash`, and it does not change the default (forward)
describe, which always resolves to the nearest tag. JSON (`--json` / `--machine`)
output is a Libra extension with no Git equivalent.

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
| Match tag pattern | `--match <glob>` (wax, repeatable) | `--match <glob>` | N/A |
| Exclude tag pattern | `--exclude <glob>` (wax, repeatable) | `--exclude <glob>` | N/A |
| Candidate count | `--candidates=<N>` / `describe.maxCandidates` (result still nearest) | `--candidates=<N>` (default 10) | N/A |
| First-parent only | `--first-parent` | `--first-parent` | N/A |
| Dirty suffix | `--dirty[=<mark>]` (tracked changes only) | `--dirty[=<mark>]` | N/A |
| Contains lookup | `--contains` → `<refname>~N` | `--contains` | N/A |
| All refs | `--all` (tags + heads + remotes, with `--contains`; partial) | `--all` | N/A |
| JSON output | `--json` with typed fields | No | No |
| Algorithm | Bounded BFS (shortest path, ≤10,000 commits) | Heuristic multi-candidate | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid revision / commit-ish | `LBR-CLI-003` | 129 |
| Invalid argument (`--match`/`--exclude` glob too long or malformed; `--candidates=0`) | `LBR-CLI-002` | 129 |
| `--abbrev=-1` and other clap parse errors | (clap) | 2 |
| `HEAD` has no commit | `LBR-REPO-003` | 128 |
| No tag/ref can describe the target and `--always` is absent | `LBR-REPO-003` | 128 |
| `--exact-match` with no exact tag | `LBR-REPO-003` | 128 |
| `--contains` with no containing ref and no `--always` | `LBR-REPO-003` | 128 |
| History walk exceeds 10,000 commits and `--always` is absent | `LBR-REPO-003` | 128 |
| Failed to read refs or objects | `LBR-IO-001` / `LBR-REPO-002` | 128 |
