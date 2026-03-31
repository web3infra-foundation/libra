# `libra tag`

Create, list, or delete tags.

## Human Output

- `libra tag -l`: prints the tag list
- `libra tag -d v1.0`: `Deleted tag 'v1.0'`
- The default create path preserves the current human-readable output

## JSON Output

`--json` / `--machine` uses `action` to distinguish operations:

```json
{
  "ok": true,
  "command": "tag",
  "data": {
    "action": "create",
    "name": "v1.0",
    "hash": "abc123...",
    "tag_type": "lightweight",
    "message": null
  }
}
```

`action=list` returns a `tags` array; `action=delete` returns `name` and `hash`.
For recovery deletes of malformed tag refs, `hash` can be `null` when the stored target is missing.

## Errors

- Tag already exists: `LBR-CONFLICT-002`
- `HEAD` has no commit to tag: `LBR-REPO-003`
- Tag not found: `LBR-CLI-003`
- Failed to write during create or delete: `LBR-IO-002`
