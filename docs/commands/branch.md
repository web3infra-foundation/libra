# `libra branch`

Create, delete, rename, inspect, or list branches.

## Human Output

- List: prints the branch list
- Safe delete: `Deleted branch feature (was abc123...)`
- Rename: `Renamed branch 'old' to 'new'`
- `--show-current`: prints the current branch name, or `HEAD detached at <hash>` when detached

## JSON Output

`--json` / `--machine` uses `action` to distinguish operations:

```json
{
  "ok": true,
  "command": "branch",
  "data": {
    "action": "create",
    "name": "feature",
    "commit": "abc123..."
  }
}
```

Supported actions:

- `list`: `branches`
- `create`: `name`, `commit`
- `delete`: `name`, `commit`, `force`
- `rename`: `old_name`, `new_name`
- `set-upstream`: `branch`, `upstream`
- `show-current`: `name`, `detached`, `commit`

## Errors

- Invalid start point or missing branch: `LBR-CLI-003`
- Current branch cannot be deleted or `HEAD` is detached: `LBR-REPO-003`
- Locked ref or branch already exists: `LBR-CONFLICT-002`
- Failed to write refs: `LBR-IO-002`
