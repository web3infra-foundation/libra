# `libra show-ref`

List local refs and their object IDs, with optional filtering by type and pattern.

## Synopsis

```
libra show-ref [OPTIONS] [<PATTERN>...]
```

## Description

`libra show-ref` enumerates references stored in the repository (branches, tags,
and optionally `HEAD`) along with the object hash each ref points to. By default
both branches and tags are shown. Use `--heads` or `--tags` to restrict output
to one category.

Positional `<PATTERN>` arguments act as substring filters on the fully-qualified
ref name (e.g., `refs/heads/main`). Only refs whose name contains at least one
of the given patterns are included. `HEAD` is never filtered out by patterns
when `--head` is specified.

Libra stores refs in SQLite rather than loose files or packed-refs, so
`show-ref` queries the database directly. This makes enumeration O(rows) with
no filesystem scanning.

## Options

| Flag | Short | Description |
|------|-------|-------------|
| `--heads` | | Show only branches (`refs/heads/`). |
| `--tags` | | Show only tags (`refs/tags/`). |
| `--head` | | Include `HEAD` in the output. |
| `--hash` | `-s` | Only show the object hash, not the reference name. |
| `<PATTERN>...` | | Filter refs by substring match on the ref name. Multiple patterns are OR-ed. |

### Examples

```bash
# List all refs
libra show-ref

# Show only branches
libra show-ref --heads

# Show only tags
libra show-ref --tags

# Include HEAD and show hashes only
libra show-ref --head --hash

# Filter to refs containing "release"
libra show-ref release

# Combine filters: only branches matching "feat"
libra show-ref --heads feat
```

## Common Commands

```bash
libra show-ref
libra show-ref --heads
libra show-ref --tags
libra show-ref --head --hash
libra show-ref --json --head --heads
libra show-ref main
```

## Human Output

Default:

```text
abc1234def5678901234567890abcdef12345678 refs/heads/main
def5678901234567890abcdef12345678abc1234 refs/tags/v1.0.0
```

With `--hash`, only the object IDs are printed:

```text
abc1234def5678901234567890abcdef12345678
def5678901234567890abcdef12345678abc1234
```

## Structured Output (JSON examples)

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "hash_only": false,
    "entries": [
      {
        "hash": "abc1234def5678901234567890abcdef12345678",
        "refname": "HEAD"
      },
      {
        "hash": "abc1234def5678901234567890abcdef12345678",
        "refname": "refs/heads/main"
      },
      {
        "hash": "def5678901234567890abcdef12345678abc1234",
        "refname": "refs/tags/v1.0.0"
      }
    ]
  }
}
```

When `--hash` is active, `hash_only` is `true`. The `entries` array is always
present regardless of the flag so that JSON consumers have a uniform schema.

## Design Rationale

### Why substring match instead of glob?

Git's `show-ref` uses prefix matching against fully-qualified ref names, but
in practice users most often want to ask "show me anything related to
`release`" or "anything with `main` in its name." Substring matching is
simpler to implement, simpler to explain, and covers the common case. It
avoids the cognitive overhead of remembering whether you need `refs/heads/main*`
or `main*`. For the rare case where you need precise control, the JSON output
gives you the full ref name array and you can filter client-side. Glob support
may be added later as a superset.

### Why SQLite-backed refs?

Git stores refs as individual files (`refs/heads/main`) and eventually packs
them into a flat `packed-refs` file. This works but has well-known scaling
problems in large monorepos: thousands of branches mean thousands of filesystem
stat calls, packed-refs rewriting is O(N) for any ref update, and concurrent
writers need lockfiles. Libra uses a `reference` table in SQLite which gives
ACID transactions, O(log N) lookups via B-tree indices, and atomic multi-ref
updates without lock contention. `show-ref` benefits directly: it is a single
`SELECT` rather than a directory walk plus packed-refs parse.

### Why is `--head` opt-in?

Following Git's convention, `HEAD` is omitted by default because it is a
symbolic ref that duplicates a `refs/heads/*` entry. Including it explicitly
with `--head` is useful when scripts need to confirm that `HEAD` is attached
and which commit it resolves to.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| List all refs | `libra show-ref` | `git show-ref` | `jj bookmark list` + `jj tag list` |
| Filter to branches | `--heads` | `--heads` | `jj bookmark list` |
| Filter to tags | `--tags` | `--tags` | `jj tag list` |
| Include HEAD | `--head` | `--head` | N/A (no HEAD concept) |
| Hash-only output | `-s` / `--hash` | `-s` / `--hash` | N/A |
| Pattern matching | Substring match | Prefix/glob match | Regex via revset |
| `--verify` (check single ref) | Not yet implemented | Yes | N/A |
| `-d` / `--dereference` | Not yet implemented | Yes | N/A |
| JSON output | `--json` | No | No |
| Ref storage | SQLite `reference` table | Loose files + packed-refs | Operation log |
| Remote-tracking refs | Not yet (TODO) | Yes (`refs/remotes/`) | Via `jj git fetch` |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| No matching refs | `LBR-CLI-003` | 129 |
| Failed to read refs | `LBR-IO-001` | 128 |
| Corrupt stored branch/tag data | `LBR-REPO-002` | 128 |
