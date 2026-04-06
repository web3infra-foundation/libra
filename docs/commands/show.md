# `libra show`

Show a commit, tag, tree, blob, or the blob referenced by `REV:path`.

## Human Output

Human mode preserves the existing presentation:

- Commit: header plus optional patch / stat / name-only output
- Annotated tag: tag metadata followed by the target object
- Tree: list of tree entries
- Blob: text content or a binary summary
- `--quiet`: validates the object reference but suppresses human output

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

Notes:

- Commit JSON `refs` are best-effort decoration metadata; unrelated branch/tag rows no longer block `show`
- Human `--quiet` still validates the target object but suppresses stdout
- Commit patch / stat paths stay strict: corrupt historical blobs fail with `LBR-REPO-002` instead of falling back to working tree contents

## Errors

- Outside a repository: `LBR-REPO-001`
- Invalid revision or missing path: `LBR-CLI-003`
- Failed to read the object: `LBR-REPO-002`
