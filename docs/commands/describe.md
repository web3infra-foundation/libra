# `libra describe`

`libra describe` finds the nearest reachable tag for a commit-ish and formats it
as `tag-N-g<abbrev>`. By default it matches annotated tags only; pass `--tags`
to include lightweight tags. When no tag matches, `--always` falls back to the
abbreviated commit hash.

## Common Commands

```bash
libra describe
libra describe --tags
libra describe --always
libra describe HEAD~1
libra describe --json
```

## Human Output

- Exact tag match: `v1.2.3`
- Reachable tag: `v1.2.3-4-gabc1234`
- `--always` fallback: `abc1234`

`--quiet` suppresses `stdout`.

## Structured Output

`--json` / `--machine` returns:

```json
{
  "ok": true,
  "command": "describe",
  "data": {
    "input": "HEAD",
    "resolved_commit": "abc1234def5678901234567890abcdef12345678",
    "result": "v1.2.3",
    "tag": "v1.2.3",
    "distance": 0,
    "abbreviated_commit": null,
    "exact_match": true,
    "used_always": false
  }
}
```

When `--always` is used and no tag matches, `tag` / `distance` are `null` and
`abbreviated_commit` contains the emitted hash.

## Errors

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid revision | `LBR-CLI-003` | 129 |
| `HEAD` has no commit | `LBR-REPO-003` | 128 |
| No tags can describe the target and `--always` is absent | `LBR-REPO-003` | 128 |
| Failed to read refs or objects | `LBR-IO-001` / `LBR-REPO-002` | 128 |
