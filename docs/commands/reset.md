# `libra reset`

Move `HEAD` and reset the index or working tree depending on the selected mode.

## Human Output

- Full reset: `HEAD is now at abc1234 <subject>`
- Pathspec reset:

```text
Unstaged changes after reset:
M	path/to/file
```

## JSON Output

```json
{
  "ok": true,
  "command": "reset",
  "data": {
    "mode": "hard",
    "commit": "abc123...",
    "short_commit": "abc1234",
    "subject": "base",
    "previous_commit": "def456...",
    "files_unstaged": 0,
    "files_restored": 1,
    "pathspecs": []
  }
}
```

When `pathspecs` is non-empty, the reset only applies to the specified paths.
`files_restored` is the number of tracked files actually rewritten or removed by `--hard`; on a clean repository, `reset --hard HEAD` can report `0`.

## Errors

- Invalid revision: `LBR-CLI-003`
- `--soft` used with pathspecs: `LBR-CLI-002`
- Corrupt index or object store: `LBR-REPO-002`
- Failed to write the index or working tree: `LBR-IO-002`
