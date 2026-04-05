# `libra shortlog`

`libra shortlog` summarizes reachable commits by author. It supports the usual
count-only and email display flags, plus an optional revision argument instead
of always reading from `HEAD`.

## Common Commands

```bash
libra shortlog
libra shortlog HEAD~5
libra shortlog -n -s
libra shortlog -e
libra shortlog --json
```

## Human Output

Default:

```text
   2  Test User
      initial
      follow-up
```

Summary mode (`-s`) suppresses subjects. `-e` appends `<email>`.
Subject extraction skips embedded signature headers and uses the first
meaningful commit message line.

## Structured Output

```json
{
  "ok": true,
  "command": "shortlog",
  "data": {
    "revision": "HEAD",
    "numbered": false,
    "summary": false,
    "email": false,
    "total_authors": 1,
    "total_commits": 2,
    "authors": [
      {
        "name": "Test User",
        "email": null,
        "count": 2,
        "subjects": ["initial", "follow-up"]
      }
    ]
  }
}
```

In summary mode, `subjects` is an empty array.

## Errors

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid `--since` / `--until` | `LBR-CLI-002` | 129 |
| Invalid revision | `LBR-CLI-003` | 129 |
| `HEAD` has no commit | `LBR-REPO-003` | 128 |
| Failed to read refs or commit graph | `LBR-IO-001` / `LBR-REPO-002` | 128 |
