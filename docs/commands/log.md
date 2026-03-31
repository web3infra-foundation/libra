# `libra log`

Show commit history. Human mode preserves the current `--oneline`, `--graph`, `--pretty`, `--stat`, `--patch`, and related output styles.

## JSON Output

`--json` / `--machine` returns a filtered, structured commit list:

```json
{
  "ok": true,
  "command": "log",
  "data": {
    "commits": [
      {
        "hash": "abc123...",
        "short_hash": "abc1234",
        "author_name": "Test User",
        "author_email": "test@example.com",
        "author_date": "2026-03-30T10:00:00+08:00",
        "committer_name": "Test User",
        "committer_email": "test@example.com",
        "committer_date": "2026-03-30T10:00:00+08:00",
        "subject": "base",
        "body": "",
        "parents": [],
        "refs": ["HEAD -> main"],
        "files": [
          { "path": "tracked.txt", "status": "added" }
        ]
      }
    ],
    "total": null
  }
}
```

Notes:

- `-n` also applies in JSON mode
- `--graph`, `--pretty`, and `--oneline` do not change the JSON schema
- `files` is always a structured change summary and never includes patch text

## Errors

- Empty branch or empty `HEAD`: `LBR-REPO-003`
- Invalid date argument: `LBR-CLI-002`
