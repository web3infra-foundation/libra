# `libra remote`

Manage configured remotes, inspect URLs, and prune stale remote-tracking refs.

## Common Commands

```bash
libra remote show
libra remote -v
libra remote add origin https://example.com/repo.git
libra remote get-url origin
libra remote get-url --all origin
libra remote set-url --add origin https://mirror.example.com/repo.git
libra remote set-url --add --push origin ssh://git@example.com/repo.git
libra remote prune --dry-run origin
```

## Human Output

- `remote show` prints configured remote names, one per line.
- `remote -v` prints every fetch URL and effective push URL:

```text
origin  https://example.com/repo.git (fetch)
origin  ssh://git@example.com/repo.git (push)
```

- `remote add` prints `Added remote 'origin' -> https://example.com/repo.git`
- `remote remove` prints `Removed remote 'origin'`
- `remote rename` prints `Renamed remote 'origin' to 'upstream'`
- `remote get-url` prints the selected URL set, one per line
- `remote set-url` prints a confirmation describing whether a URL was added, replaced, or deleted
- `remote prune` prints each pruned branch and a final summary; `--dry-run` uses `[would prune]`

## Structured Output

- `--json` writes one success envelope to `stdout`
- `--machine` writes the same schema as compact single-line JSON
- action-specific payloads are tagged with `data.action`

### Action Schemas

- `add`: `name`, `url`
- `remove`: `name`
- `rename`: `old_name`, `new_name`
- `list`: `verbose`, `remotes[]`
- `urls`: `name`, `push`, `all`, `urls[]`
- `set-url`: `name`, `role`, `mode`, `urls[]`, `removed`
- `prune`: `name`, `dry_run`, `stale_branches[]`

### Schema Notes

- `list.remotes[].fetch_urls` contains all configured fetch URLs
- `list.remotes[].push_urls` contains effective push URLs; when no explicit `pushurl` is configured it falls back to fetch URLs
- `prune.stale_branches[].branch` is the user-facing short name such as `origin/feature`
- `remote show` currently maps to `action = "list"` with `verbose = false`

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Duplicate remote name | `LBR-CONFLICT-002` | 128 |
| Missing remote / missing URL / unmatched `set-url --delete` pattern | `LBR-CLI-003` | 129 |
| Failed to read remote config | `LBR-IO-001` | 128 |
| Failed to update remote config or prune ref | `LBR-IO-002` | 128 |
| Remote discovery / auth / network failure during prune | fetch-aligned network/auth codes | 128 |
| Remote object format mismatch during prune | `LBR-REPO-003` | 128 |
