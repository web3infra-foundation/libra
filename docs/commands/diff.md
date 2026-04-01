# `libra diff`

Compare differences between `HEAD`, the index, the working tree, or two revisions.

## Human Output

Supported output:

- Default unified diff
- `--name-only`
- `--name-status`
- `--numstat`
- `--stat`

`--output <file>` writes human-readable output to a file; in `--json` mode this flag is ignored and output always goes to stdout.

## JSON Output

```json
{
  "ok": true,
  "command": "diff",
  "data": {
    "old_ref": "index",
    "new_ref": "working tree",
    "files": [
      {
        "path": "tracked.txt",
        "status": "modified",
        "insertions": 1,
        "deletions": 0,
        "hunks": [
          {
            "old_start": 1,
            "old_lines": 1,
            "new_start": 1,
            "new_lines": 2,
            "lines": [" tracked", "+updated"]
          }
        ]
      }
    ],
    "total_insertions": 1,
    "total_deletions": 0,
    "files_changed": 1
  }
}
```

## Errors

- Invalid revision: `LBR-CLI-003`
- Failed to read the index or object store: `LBR-REPO-002`
- Failed to read a file: `LBR-IO-001`
- Failed to write the output file: `LBR-IO-002`
