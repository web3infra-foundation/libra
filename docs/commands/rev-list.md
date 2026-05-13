# `libra rev-list`

List commit objects reachable from a revision.

## Synopsis

```bash
libra rev-list [<SPEC>]
```

## Description

`libra rev-list` resolves a revision input to a commit, walks the reachable history, and prints commit IDs newest first. When `<SPEC>` is omitted, the command defaults to `HEAD`.

## Options

| Flag | Description |
|------|-------------|
| `<SPEC>` | Revision to enumerate from. Defaults to `HEAD`. |

## Common Commands

```bash
libra rev-list
libra rev-list HEAD
libra rev-list HEAD~1
libra rev-list refs/remotes/origin/main
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
| JSON output | `--json` | No | No |
| Ordering | Newest first | Reachability order | Revset-dependent |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid target ref | `LBR-CLI-003` | 129 |
| Failed to read repository metadata | `LBR-IO-001` | 128 |
| Corrupt stored refs/objects | `LBR-REPO-002` | 128 |
