# `libra show-ref`

`libra show-ref` lists local refs and their object IDs. It supports branch,
tag, and `HEAD` filtering plus structured JSON output.

## Common Commands

```bash
libra show-ref
libra show-ref --heads
libra show-ref --tags
libra show-ref --head --hash
libra show-ref --json --head --heads
```

## Human Output

Default:

```text
abc1234def5678901234567890abcdef12345678 refs/heads/main
```

With `--hash`, only the object IDs are printed.

## Structured Output

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "hash_only": false,
    "entries": [
      {
        "hash": "abc1234def5678901234567890abcdef12345678",
        "refname": "HEAD"
      },
      {
        "hash": "abc1234def5678901234567890abcdef12345678",
        "refname": "refs/heads/main"
      }
    ]
  }
}
```

## Errors

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| No matching refs | `LBR-CLI-003` | 129 |
| Failed to read refs | `LBR-IO-001` | 128 |
| Corrupt stored branch/tag data | `LBR-REPO-002` | 128 |
