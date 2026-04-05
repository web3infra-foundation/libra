# `libra cat-file`

`libra cat-file` inspects Git objects and selected AI objects. Libra supports
JSON output for `-t`, `-s`, `-p`, and the `--ai*` inspection modes. `-e` stays
human-only.

## Common Commands

```bash
libra cat-file -t HEAD
libra cat-file -s HEAD
libra cat-file -p HEAD
libra cat-file -t HEAD --json
libra cat-file --ai-list-types --json
```

## Structured Output

Type mode:

```json
{
  "ok": true,
  "command": "cat-file",
  "data": {
    "mode": "type",
    "object": "HEAD",
    "hash": "abc1234def5678901234567890abcdef12345678",
    "object_type": "commit"
  }
}
```

Size mode returns `mode = "size"` and `size`. Pretty-print mode returns a
mode-specific payload for commits, trees, blobs, or tags.

## Notes

- `cat-file -e` does not support `--json` / `--machine`
- Blob/tag pretty-print JSON requires UTF-8 content; non-text payloads still
  fail explicitly instead of returning lossy data

## Errors

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid object / revision | `LBR-CLI-003` | 129 |
| Unsupported argument combination | `LBR-CLI-002` | 129 |
| Failed to read object data | `LBR-IO-001` / `LBR-REPO-002` | 128 |
