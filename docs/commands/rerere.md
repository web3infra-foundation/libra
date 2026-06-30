# `libra rerere`

**RE**use **RE**corded **RE**solution. Records how you resolved a merge conflict
and replays that resolution automatically when the identical conflict reappears.

## Synopsis

```
libra rerere [status | diff | forget <path>... | clear | gc]
```

## Description

With no subcommand, `rerere` scans the tracked files for conflict markers and:

- records a **preimage** (the conflicted file) for each new conflict, tracking
  it in `.libra/rerere/MERGE_RR`;
- if a recorded **postimage** (resolution) already matches a conflict, **replays**
  it — writing the resolved content back to the file;
- once a tracked conflict has been resolved by hand, records its postimage so
  the next identical conflict resolves itself.

A conflict is matched by the SHA-256 of the conflicted file's bytes, so a
resolution replays when the whole conflicted file is byte-identical to one seen
before.

| Subcommand | Description |
|------------|-------------|
| (none) | Record preimages / replay resolutions / record postimages. |
| `status` | List the paths whose conflicts are currently tracked. |
| `diff` | Show what changed in each tracked file since its preimage was recorded. |
| `forget <path>...` | Drop the recorded resolution for the given paths. |
| `clear` | Stop tracking the current conflicts (recorded resolutions are kept). |
| `gc` | Prune recorded resolutions older than the thresholds (60 days resolved / 15 days unresolved). |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. |
| `128` | Not inside a repository, `forget` of a path with no recording, or an I/O error. |

## Examples

```bash
# After a merge leaves conflicts, record them
libra rerere

# Resolve the files by hand, then let rerere learn the resolution
libra rerere

# The next time the same conflict appears, rerere resolves it for you
libra rerere status
```

## Comparison with Git

| Task | Libra | Git |
|------|-------|-----|
| Record / replay | `libra rerere` | `git rerere` |
| Inspect | `libra rerere status` / `diff` | `git rerere status` / `diff` |
| Drop / reset | `libra rerere forget <p>` / `clear` / `gc` | `git rerere forget <p>` / `clear` / `gc` |

Differences and deferred features: matching is whole-file byte-identical (Git
normalises each conflict hunk and is independent of ours/theirs order); and the
**automatic** integration with `merge` / `rebase` / `cherry-pick` (via
`rerere.enabled` and `--rerere-autoupdate`) is a documented follow-up — for now
run `libra rerere` explicitly. The `--rerere-autoupdate` flags on those commands
are still accepted as no-ops.
