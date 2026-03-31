# `libra show`

Show a commit, tag, tree, blob, or the blob referenced by `REV:path`.

## Human Output

Human mode preserves the existing presentation:

- Commit: header plus optional patch / stat / name-only output
- Annotated tag: tag metadata followed by the target object
- Tree: list of tree entries
- Blob: text content or a binary summary

## JSON Output

`data.type` determines the schema:

- `commit`
- `tag`
- `tree`
- `blob`

Example:

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "commit",
    "hash": "abc123...",
    "short_hash": "abc1234",
    "subject": "base",
    "files": [
      { "path": "tracked.txt", "status": "added" }
    ]
  }
}
```

## Errors

- Invalid revision or missing path: `LBR-CLI-003`
- Failed to read the object: `LBR-REPO-002`
