# `libra rev-list`

List commit objects reachable from a revision.

## Synopsis

```bash
libra rev-list [OPTIONS] [SPEC]... [-- <PATH>...]
```

## Description

`libra rev-list` resolves one or more revision inputs to commits, walks the reachable history, applies optional exclusion/range, first-parent, author, committer, message grep, path, time-window, parent-count, and count/limit filters, and prints commit IDs newest first. When `<SPEC>` is omitted, the command defaults to `HEAD`. Output formatting can include parent commit IDs (`--parents`) and committer timestamps (`--timestamp`).

## Options

| Flag | Description |
|------|-------------|
| `-n <N>`, `--max-count <N>` | Limit output to at most `N` commits after sorting. |
| `--skip <N>` | Skip the first `N` commits before output or counting. |
| `--count` | Print only the number of commits after filters. |
| `--since <DATE>`, `--after <DATE>` | Print commits whose committer timestamp is at or after `DATE`. |
| `--until <DATE>`, `--before <DATE>` | Print commits whose committer timestamp is at or before `DATE`. |
| `--merges` | Print only commits with at least two parents. |
| `--no-merges` | Omit commits with at least two parents. |
| `--min-parents <N>` | Print only commits with at least `N` parents. |
| `--max-parents <N>` | Print only commits with at most `N` parents. |
| `--no-min-parents` | Clear the lower parent-count bound. |
| `--no-max-parents` | Clear the upper parent-count bound. |
| `--first-parent` | Follow only the first parent when walking through merge commits. |
| `--author <PATTERN>` | Print only commits whose author name or email contains `PATTERN` case-insensitively. |
| `--committer <PATTERN>` | Print only commits whose committer name or email contains `PATTERN` case-insensitively. |
| `--grep <PATTERN>` | Print only commits whose message matches `PATTERN` as a case-sensitive regular expression. May be repeated; any matching pattern includes the commit. |
| `--parents` | Print parent commit IDs after each listed commit. |
| `--timestamp` | Prefix each listed commit with its committer timestamp, matching Git's `timestamp commit [parents...]` field order. |
| `<SPEC>...` | Revisions to enumerate from. Defaults to `HEAD`; accepts multiple positive revisions, `^<rev>` exclusions, `A..B`, and `A...B`. |
| `-- <PATH>...` | Limit commits to changes that touched one of the listed paths. |

## Common Commands

```bash
libra rev-list
libra rev-list HEAD
libra rev-list --count HEAD
libra rev-list -n 5 HEAD
libra rev-list --skip 5 --max-count 10 HEAD
libra rev-list --since 2026-01-01 HEAD
libra rev-list --after "2 weeks ago" --before 2026-06-01 HEAD
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
libra rev-list --first-parent HEAD
libra rev-list --author alice HEAD
libra rev-list --committer alice HEAD
libra rev-list --grep 'fix|feat' HEAD
libra rev-list HEAD -- src/
libra rev-list --parents HEAD
libra rev-list --timestamp --parents HEAD
libra rev-list HEAD~1
libra rev-list refs/remotes/origin/main
libra --json rev-list HEAD
```

## Human Output

Output is one commit ID per line by default. Multiple positive revisions are unioned and de-duplicated. `^<rev>` excludes commits reachable from that revision. `A..B` is equivalent to `^A B`; `A...B` prints the symmetric difference between both sides. `--first-parent` limits traversal through merge commits to the first parent chain. Author, committer, message grep, path, time-window, and parent-count filters are applied before `--skip`, `--max-count`, and `--count`. `--author` and `--committer` match the respective `name <email>` string case-insensitively. `--grep` matches the full commit message with a case-sensitive regular expression; repeated `--grep` patterns use OR semantics. Path filters must follow an explicit `--` separator and match files or directories relative to the worktree root. `--since`/`--after` and `--until`/`--before` accept `YYYY-MM-DD`, RFC3339/full timestamps with timezone, Unix timestamps, and relative forms such as `2 weeks ago`. With `--parents`, each line becomes `commit parent...`. With `--timestamp`, each line becomes `timestamp commit`; combining both produces `timestamp commit parent...`. With `--count`, output remains a single decimal count and ignores output-format flags.

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
    "first_parent": false,
    "author": null,
    "committer": null,
    "grep": [],
    "pathspecs": [],
    "since": null,
    "until": null,
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
    "first_parent": false,
    "author": null,
    "committer": null,
    "grep": [],
    "pathspecs": [],
    "since": null,
    "until": null,
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
| Time filters | `--since` / `--after`, `--until` / `--before` | Same | revset predicates |
| Parent-count filters | `--merges`, `--no-merges`, `--min-parents`, `--max-parents`, `--no-min-parents`, `--no-max-parents` | Same | revset predicates |
| First-parent traversal | `--first-parent` | Same | revset/graph predicates |
| Author filter | `--author <PATTERN>` | Same | revset predicates |
| Committer filter | `--committer <PATTERN>` | Same | revset predicates |
| Message grep | `--grep <PATTERN>` | Same | revset predicates |
| Path limitation | `-- <PATH>...` | Same | revset/file predicates |
| Parent output | `--parents` | Same | revset/template output |
| Timestamp output | `--timestamp` | Same | template output |
| JSON output | `--json` | No | No |
| Ordering | Newest first | Reachability order | Revset-dependent |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid target ref | `LBR-CLI-003` | 129 |
| Invalid date filter | `LBR-CLI-002` | 129 |
| Invalid grep regex | `LBR-CLI-002` | 129 |
| Failed to read repository metadata | `LBR-IO-001` | 128 |
| Corrupt stored refs/objects | `LBR-REPO-002` | 128 |
