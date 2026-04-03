# `libra log`

Show commit history. Human mode preserves the current `--oneline`, `--graph`, `--pretty`, `--stat`, `--patch`, and related output styles.
`--quiet` suppresses human output but still validates the requested history range.

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
    "total": 1
  }
}
```

Notes:

- `-n` also applies in JSON mode
- `total` is `null` only when `-n` truncates the result set; otherwise it reflects the filtered commit count
- `--graph`, `--pretty`, and `--oneline` do not change the JSON schema
- `--decorate` only affects human rendering; JSON always returns a `refs` array, and auxiliary ref metadata is collected best-effort
- `files` is always a structured change summary and never includes patch text

## Errors

- Outside a repository: `LBR-REPO-001`
- Empty branch or empty `HEAD`: `LBR-REPO-003`
- Invalid date argument: `LBR-CLI-002`
- Invalid `--decorate` option: `LBR-CLI-002`
- Failed to read historical tree/blob objects for `--patch` / `--stat`: `LBR-REPO-002`
