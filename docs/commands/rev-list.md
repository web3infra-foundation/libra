# `libra rev-list`

List commit objects reachable from a revision.

## Synopsis

```bash
libra rev-list [OPTIONS] [SPEC]...
```

## Description

`libra rev-list` resolves one or more revision inputs to commits, walks the reachable history, applies optional exclusion/range, parent-count, and count/limit filters, and prints commit IDs newest first. When `<SPEC>` is omitted, the command defaults to `HEAD`. Output formatting can include parent commit IDs (`--parents`) and committer timestamps (`--timestamp`).

## Options

| Flag | Description |
|------|-------------|
| `-n <N>`, `--max-count <N>` | Limit output to at most `N` commits after sorting. |
| `--skip <N>` | Skip the first `N` commits before output or counting. |
| `--count` | Print only the number of commits after filters. |
| `--merges` | Print only commits with at least two parents. |
| `--no-merges` | Omit commits with at least two parents. |
| `--min-parents <N>` | Print only commits with at least `N` parents. |
| `--max-parents <N>` | Print only commits with at most `N` parents. |
| `--no-min-parents` | Clear the lower parent-count bound. |
| `--no-max-parents` | Clear the upper parent-count bound. |
| `--parents` | Print parent commit IDs after each listed commit. |
| `--timestamp` | Prefix each listed commit with its committer timestamp, matching Git's `timestamp commit [parents...]` field order. |
| `<SPEC>...` | Revisions to enumerate from. Defaults to `HEAD`; accepts multiple positive revisions, `^<rev>` exclusions, `A..B`, and `A...B`. |

## Common Commands

```bash
libra rev-list
libra rev-list HEAD
libra rev-list --count HEAD
libra rev-list -n 5 HEAD
libra rev-list --skip 5 --max-count 10 HEAD
libra rev-list main feature
libra rev-list ^main feature
libra rev-list main..feature
libra rev-list main...feature
libra rev-list --merges HEAD
libra rev-list --no-merges HEAD
libra rev-list --min-parents 1 --max-parents 1 HEAD
libra rev-list --min-parents 1 --no-min-parents HEAD
libra rev-list --max-parents 0 HEAD
libra rev-list --max-parents 0 --no-max-parents HEAD
libra rev-list --parents HEAD
libra rev-list --timestamp --parents HEAD
libra rev-list HEAD~1
libra rev-list refs/remotes/origin/main
libra --json rev-list HEAD
```

## Human Output

Output is one commit ID per line by default. Multiple positive revisions are unioned and de-duplicated. `^<rev>` excludes commits reachable from that revision. `A..B` is equivalent to `^A B`; `A...B` prints the symmetric difference between both sides. Parent-count filters are applied before `--skip`, `--max-count`, and `--count`. With `--parents`, each line becomes `commit parent...`. With `--timestamp`, each line becomes `timestamp commit`; combining both produces `timestamp commit parent...`. With `--count`, output remains a single decimal count and ignores output-format flags.

```text
abc1234def5678901234567890abcdef12345678
def5678901234567890abcdef12345678abc1234
```

```text
1715788800 abc1234def5678901234567890abcdef12345678 def5678901234567890abcdef12345678abc1234
1715702400 def5678901234567890abcdef12345678abc1234
```

## Structured Output

```json
{
  "ok": true,
  "command": "rev-list",
  "data": {
    "input": "HEAD",
    "inputs": ["HEAD"],
    "commits": [
      "abc1234def5678901234567890abcdef12345678",
      "def5678901234567890abcdef12345678abc1234"
    ],
    "total": 2,
    "count_only": false,
    "parents": false,
    "timestamp": false,
    "merges": false,
    "no_merges": false,
    "min_parents": null,
    "max_parents": null,
    "no_min_parents": false,
    "no_max_parents": false,
    "max_count": null,
    "skip": 0
  }
}
```

When `--parents` or `--timestamp` is present, `commits[]` remains the plain commit-ID list for compatibility and `entries[]` carries the optional metadata used for human output.

```json
{
  "ok": true,
  "command": "rev-list",
  "data": {
    "input": "HEAD",
    "inputs": ["HEAD"],
    "commits": [
      "abc1234def5678901234567890abcdef12345678"
    ],
    "entries": [
      {
        "commit": "abc1234def5678901234567890abcdef12345678",
        "parents": [
          "def5678901234567890abcdef12345678abc1234"
        ],
        "timestamp": 1715788800
      }
    ],
    "total": 1,
    "count_only": false,
    "parents": true,
    "timestamp": true,
    "merges": false,
    "no_merges": false,
    "min_parents": null,
    "max_parents": null,
    "no_min_parents": false,
    "no_max_parents": false,
    "max_count": 1,
    "skip": 0
  }
}
```

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Default target | `HEAD` | `HEAD` | current revision |
| Revision navigation | `HEAD~1`, tags, remote refs | Same | revsets |
| Multiple revisions | Supported, de-duplicated | Same | revsets |
| Exclusion/range syntax | `^A`, `A..B`, `A...B` | Same | revsets |
| Count and limit | `--count`, `-n` / `--max-count`, `--skip` | Same | revset functions |
| Parent-count filters | `--merges`, `--no-merges`, `--min-parents`, `--max-parents`, `--no-min-parents`, `--no-max-parents` | Same | revset predicates |
| Parent output | `--parents` | Same | revset/template output |
| Timestamp output | `--timestamp` | Same | template output |
| JSON output | `--json` | No | No |
| Ordering | Newest first | Reachability order | Revset-dependent |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid target ref | `LBR-CLI-003` | 129 |
| Failed to read repository metadata | `LBR-IO-001` | 128 |
| Corrupt stored refs/objects | `LBR-REPO-002` | 128 |
