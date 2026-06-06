# `libra rev-list`

List commit objects reachable from a revision.

## Synopsis

```bash
libra rev-list [OPTIONS] [SPEC]...
```

## Description

`libra rev-list` resolves one or more revision inputs to commits, walks the reachable history, and prints commit IDs newest first. When no `<SPEC>` is given, the command defaults to `HEAD`.

Multiple specs are unioned and de-duplicated. Exclusions and ranges follow Git:

- `^<rev>` — exclude everything reachable from `<rev>`.
- `A..B` — commits reachable from `B` but not `A` (sugar for `B ^A`).
- `A...B` — the symmetric difference: commits reachable from `A` or `B` but not from their merge base(s). All best merge bases (including criss-cross) are excluded; with no common ancestor it degrades to `A B`.

## Options

| Flag | Description |
|------|-------------|
| `<SPEC>...` | Revisions to enumerate from (default `HEAD`). Supports `^<rev>`, `A..B`, `A...B`. |
| `-n`, `--max-count <N>` | Limit output to the N newest commits. |
| `--skip <N>` | Skip the first N commits of the filtered output. |
| `--count` | Print only the number of commits (text mode); JSON still emits the full envelope. |
| `--merges` | Show only merge commits (≥2 parents). Conflicts with `--no-merges`. |
| `--no-merges` | Show only non-merge commits (<2 parents). |
| `--min-parents <N>` | Show only commits with at least N parents. |
| `--max-parents <N>` | Show only commits with at most N parents. |
| `--parents` | Append each commit's parent hashes: `<hash> <p1> <p2>…`. |
| `--timestamp` | Prefix each line with the committer Unix timestamp. |

Predicates apply before `--skip`/`--max-count`, so `--skip`/`-n` count post-filter commits (matching Git).

## Common Commands

```bash
libra rev-list
libra rev-list HEAD
libra rev-list HEAD~1
libra rev-list main..HEAD              # commits on HEAD but not main
libra rev-list HEAD ^origin/main       # local-only commits
libra rev-list -n 10 --no-merges HEAD  # 10 newest non-merge commits
libra rev-list --count HEAD            # just the count
libra rev-list --parents --timestamp HEAD
libra --json rev-list HEAD
```

## Human Output

Output is one commit ID per line.

```text
abc1234def5678901234567890abcdef12345678
def5678901234567890abcdef12345678abc1234
```

## Structured Output

```json
{
  "ok": true,
  "command": "rev-list",
  "data": {
    "input": "HEAD",
    "commits": [
      "abc1234def5678901234567890abcdef12345678",
      "def5678901234567890abcdef12345678abc1234"
    ],
    "total": 2
  }
}
```

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Default target | `HEAD` | `HEAD` | current revision |
| Revision navigation | `HEAD~1`, tags, remote refs | Same | revsets |
| Multi-spec / exclusion | `A B ^C` | Same | revsets |
| Ranges | `A..B`, `A...B` | Same | `A..B` (revset) |
| Limit / skip | `-n`/`--max-count`, `--skip` | Same | `-n` |
| Count only | `--count` | `--count` | N/A |
| Parent filters | `--merges`/`--no-merges`/`--min-parents`/`--max-parents` | Same | revset functions |
| Format | `--parents`, `--timestamp` | Same | `--template` |
| JSON output | `--json` | No | No |
| Object walk | Not implemented (deferred) | `--objects` | N/A |
| Topo / date order | Commit-date (default); `--topo-order`, `--since`/`--until`, `--children`, `--header`, pathspec limiting not implemented | All supported | Revset-dependent |
| Ordering | Newest first (commit date) | Reachability order | Revset-dependent |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid target ref | `LBR-CLI-003` | 129 |
| Failed to read repository metadata | `LBR-IO-001` | 128 |
| Corrupt stored refs/objects | `LBR-REPO-002` | 128 |
