# `libra rev-list`

List commit objects reachable from a revision.

## Synopsis

```bash
libra rev-list [OPTIONS] [SPEC]
```

## Description

`libra rev-list` resolves a revision input to a commit, walks the reachable history, applies optional count/limit filters, and prints commit IDs newest first. When `<SPEC>` is omitted, the command defaults to `HEAD`. Output formatting can include parent commit IDs (`--parents`) and committer timestamps (`--timestamp`).

## Options

| Flag | Description |
|------|-------------|
| `-n <N>`, `--max-count <N>` | Limit output to at most `N` commits after sorting. |
| `--skip <N>` | Skip the first `N` commits before output or counting. |
| `--count` | Print only the number of commits after filters. |
| `--parents` | Print parent commit IDs after each listed commit. |
| `--timestamp` | Prefix each listed commit with its committer timestamp, matching Git's `timestamp commit [parents...]` field order. |
| `<SPEC>` | Revision to enumerate from. Defaults to `HEAD`. |

## Common Commands

```bash
libra rev-list
libra rev-list HEAD
libra rev-list --count HEAD
libra rev-list -n 5 HEAD
libra rev-list --skip 5 --max-count 10 HEAD
libra rev-list --parents HEAD
libra rev-list --timestamp --parents HEAD
libra rev-list HEAD~1
libra rev-list refs/remotes/origin/main
libra --json rev-list HEAD
```

## Human Output

Output is one commit ID per line by default. With `--parents`, each line becomes `commit parent...`. With `--timestamp`, each line becomes `timestamp commit`; combining both produces `timestamp commit parent...`. With `--count`, output remains a single decimal count and ignores output-format flags.

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
    "commits": [
      "abc1234def5678901234567890abcdef12345678",
      "def5678901234567890abcdef12345678abc1234"
    ],
    "total": 2,
    "count_only": false,
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
| Count and limit | `--count`, `-n` / `--max-count`, `--skip` | Same | revset functions |
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
