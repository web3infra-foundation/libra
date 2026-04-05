# `libra clean`

`libra clean` removes untracked files from the working tree. Libra requires an
explicit mode: `-n` for preview or `-f` for deletion.

## Common Commands

```bash
libra clean -n
libra clean -f
libra clean -n --json
```

## Human Output

Dry-run:

```text
Would remove build/output.log
Would remove notes.txt
```

Forced removal:

```text
Removing build/output.log
Removing notes.txt
```

`--quiet` suppresses `stdout`.

## Structured Output

```json
{
  "ok": true,
  "command": "clean",
  "data": {
    "dry_run": true,
    "removed": ["build/output.log", "notes.txt"]
  }
}
```

`removed` is empty when there is nothing to clean.

## Errors

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Missing `-f` / `-n` | `LBR-CLI-002` | 129 |
| Corrupted index or untracked scan failure | `LBR-IO-001` | 128 |
| Path resolves outside the worktree | `LBR-CONFLICT-002` | 128 |
| File deletion failed | `LBR-IO-002` | 128 |
