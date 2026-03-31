# `libra switch`

Switch branches, create and switch to a new branch, or move to a detached `HEAD`.

## Human Output

- Switch branch: `Switched to branch 'main'`
- Create and switch: `Switched to a new branch 'feature'`
- Detached: `HEAD is now at abc1234`
- `--track` prints the upstream configuration message before the switch result

## JSON Output

`--json` / `--machine` returns:

```json
{
  "ok": true,
  "command": "switch",
  "data": {
    "previous_branch": "main",
    "previous_commit": "abc123...",
    "branch": "feature",
    "commit": "abc123...",
    "created": true,
    "detached": false,
    "tracking": null
  }
}
```

`tracking` is only present with `--track` and includes `remote` and `remote_branch`.

## Errors

- Missing branch name or missing `--track` remote branch argument: `LBR-CLI-002`
- Invalid switch target, invalid revision, or missing remote-tracking branch: `LBR-CLI-003`
- Dirty working tree: `LBR-REPO-003`
- Failed to update `HEAD` or restore the working tree: `LBR-IO-002`
