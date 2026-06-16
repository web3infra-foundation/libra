# `libra show-ref`

List local refs and their object IDs, with optional filtering by type and pattern.

## Synopsis

```
libra show-ref [OPTIONS] [PATTERN]...
```

## Description

`libra show-ref` enumerates references stored in the repository (branches, tags,
and optionally `HEAD`) along with the object hash each ref points to. By default
both branches and tags are shown. Use `--heads` / `--branches` or `--tags` to
restrict output to one category.

Git-compatible reset aliases are accepted for scripts that compose flags:
`--no-branches`, `--no-tags`, `--no-head`, `--no-dereference`,
`--no-abbrev`, `--no-verify`, and `--no-exists` reset their corresponding
positive flag when they appear later on the command line. Git also accepts
`--no-hash` as a hash-only spelling; Libra follows that behavior.

Positional `<PATTERN>` arguments match complete path segments from the end of
the fully-qualified ref name, following Git's `show-ref` behavior. For example,
`main` matches `refs/heads/main` and `refs/remotes/origin/main`, but does not
match `refs/heads/main-2`. `HEAD` is never filtered out by patterns when
`--head` is specified.

Use `-d` / `--dereference` to peel annotated tags. Annotated tags are printed
twice: once for the tag object itself and once as `refs/tags/<name>^{}` pointing
at the peeled target. Lightweight tags remain single-line entries.

Use `--abbrev[=<n>]` to shorten displayed object IDs, defaulting to 7 hex
digits when the value is omitted. `--abbrev=0` keeps full hashes. `--hash=<n>`
combines hash-only output with the same width control; `--hash` without a value
keeps the full hash unless `--abbrev` is also supplied.

Use `--verify <ref>` when a script needs an exact refname such as `HEAD` or
`refs/heads/main`; short names like `main` are rejected. Use `--exists <ref>` to
check whether exactly one ref exists without printing a success line.

Use `--exclude-existing[=<pattern>]` as a Git-compatible stdin filter. Each
input line is parsed for the final whitespace-separated refname, a trailing
`^{}` peel suffix is ignored for the existence check, refs already present in
the local repository are dropped, and missing refs are printed back exactly as
they appeared on stdin. When `<pattern>` is supplied, only refnames with that
prefix are considered.

Libra stores refs in SQLite rather than loose files or packed-refs, so
`show-ref` queries the database directly. This makes enumeration O(rows) with
no filesystem scanning.

## Options

| Flag | Short | Description |
|------|-------|-------------|
| `--heads` | | Show only branches (`refs/heads/`). |
| `--branches` | | Git-compatible alias for `--heads`. |
| `--no-branches` | | Reset `--heads` / `--branches` scope filtering. |
| `--tags` | | Show only tags (`refs/tags/`). |
| `--no-tags` | | Reset `--tags` scope filtering. |
| `--head` | | Include `HEAD` in the output. |
| `--no-head` | | Reset `--head` so `HEAD` is omitted. |
| `--hash[=<n>]` | `-s[<n>]` | Only show the object hash, optionally shortened to `n` hex digits. |
| `--no-hash` | | Git-compatible hash-only alias. |
| `--abbrev[=<n>]` | | Abbreviate object IDs to `n` hex digits, or 7 when `n` is omitted. |
| `--no-abbrev` | | Reset `--abbrev` and display full object IDs. |
| `--dereference` | `-d` | Dereference annotated tags and include peeled `^{}` entries. |
| `--no-dereference` | | Reset `--dereference` and suppress peeled tag entries. |
| `--verify` | | Verify exact refnames instead of pattern filtering. |
| `--no-verify` | | Reset `--verify` and return to pattern filtering. |
| `--exists` | | Check whether exactly one ref exists without printing it. |
| `--no-exists` | | Reset `--exists` and return to normal ref listing. |
| `--exclude-existing[=<pattern>]` | | Filter stdin to refs that do not already exist locally. |
| `<PATTERN>...` | | Filter refs by path-segment suffix match on the ref name. Multiple patterns are OR-ed. |

### Examples

```bash
# List all refs
libra show-ref

# Show only branches
libra show-ref --heads

# Same branch filter using Git's alias
libra show-ref --branches

# Reset a composed branch-only filter back to default branch+tag listing
libra show-ref --branches --no-branches

# Show only tags
libra show-ref --tags

# Reset an abbreviated display back to full object IDs
libra show-ref --abbrev=12 --no-abbrev --heads

# Peel annotated tags
libra show-ref --dereference --tags v1.0

# Include HEAD and show hashes only
libra show-ref --head --hash

# Abbreviate refs to 12 hex digits
libra show-ref --abbrev=12 --heads

# Print only 12 hex digits per matching hash
libra show-ref --hash=12 --heads

# Verify an exact ref
libra show-ref --verify refs/heads/main

# Check existence without success output
libra show-ref --exists refs/heads/main

# Keep only refs from stdin that are missing locally
printf '%s\n' 'abc123 refs/heads/new' | libra show-ref --exclude-existing

# Filter to refs ending in the path segment "release"
libra show-ref release

# Combine filters: only branches matching "feat"
libra show-ref --heads feat
```

## Common Commands

```bash
libra show-ref
libra show-ref --heads
libra show-ref --branches
libra show-ref --branches --no-branches
libra show-ref --tags
libra show-ref --dereference --tags v1.0
libra show-ref --dereference --no-dereference --tags v1.0
libra show-ref --head --hash
libra show-ref --abbrev=12 --heads
libra show-ref --abbrev=12 --no-abbrev --heads
libra show-ref --hash=12 --heads
libra show-ref --no-hash --heads
libra show-ref --verify refs/heads/main
libra show-ref --verify --no-verify main
libra show-ref --exists refs/heads/main
libra show-ref --exists --no-exists refs/heads/main
libra show-ref --exclude-existing
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

With `--dereference`, annotated tags include an additional peeled entry:

```text
def5678901234567890abcdef12345678abc1234 refs/tags/v1.0.0
abc1234def5678901234567890abcdef12345678 refs/tags/v1.0.0^{}
```

With `--abbrev=12`, hashes are shortened while refnames remain visible:

```text
abc1234def56 refs/heads/main
```

With `--hash=12`, only the shortened hash is printed:

```text
abc1234def56
```

## Structured Output (JSON examples)

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "hash_only": false,
    "abbrev": null,
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

When `--hash` is active, `hash_only` is `true`. When `--abbrev` or a hash width
is active, `abbrev` records the width and `entries[].hash` contains the displayed
abbreviated value. The `entries` array is always present regardless of the flag
so that JSON consumers have a uniform schema.

With `--exists`, human output is silent on success. JSON output reports the
checked ref:

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "exists": true,
    "refname": "refs/heads/main"
  }
}
```

With `--exclude-existing`, human output preserves each missing input line. JSON
output reports the parsed refname alongside the preserved line:

```json
{
  "ok": true,
  "command": "show-ref",
  "data": {
    "exclude_existing": true,
    "pattern": "refs/heads",
    "entries": [
      {
        "line": "abc123 refs/heads/new",
        "refname": "refs/heads/new"
      }
    ]
  }
}
```

## Design Rationale

### Why path-segment suffix matching?

Git's `show-ref` pattern matching treats patterns as complete refname path
segments matched from the end of the fully-qualified ref name. Libra follows
that behavior so scripts can pass `main` without accidentally matching
`main-2`, while still matching both `refs/heads/main` and
`refs/remotes/origin/main`.

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
| Filter to branches | `--heads` / `--branches` | `--heads` / `--branches` | `jj bookmark list` |
| Filter to tags | `--tags` | `--tags` | `jj tag list` |
| Reset composed filters | `--no-branches`, `--no-tags`, `--no-head`, `--no-dereference`, `--no-abbrev`, `--no-verify`, `--no-exists` | Same | N/A |
| Include HEAD | `--head` | `--head` | N/A (no HEAD concept) |
| Hash-only output | `-s[<n>]` / `--hash[=<n>]` / `--no-hash` | `-s[<n>]` / `--hash[=<n>]` / `--no-hash` | N/A |
| Abbreviate object IDs | `--abbrev[=<n>]` | `--abbrev[=<n>]` | N/A |
| Dereference annotated tags | `-d` / `--dereference` | `-d` / `--dereference` | N/A |
| Pattern matching | Path-segment suffix match | Path-segment suffix match | Regex via revset |
| `--verify` (check exact ref) | `--verify <ref>` | Yes | N/A |
| `--exists` (existence check) | `--exists <ref>` | Yes | N/A |
| `--exclude-existing` stdin filter | `--exclude-existing[=<pattern>]` | Yes | N/A |
| JSON output | `--json` | No | No |
| Ref storage | SQLite `reference` table | Loose files + packed-refs | Operation log |
| Remote-tracking refs | Yes (`refs/remotes/`) | Yes (`refs/remotes/`) | Via `jj git fetch` |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| No matching refs | `LBR-CLI-003` | 129 |
| `--verify` target is not an exact existing ref | `LBR-CLI-003` | 128, or 1 with global `--quiet` |
| `--exists` target is missing | `LBR-CLI-003` | 2 |
| `--exclude-existing` combined with `--verify` / `--exists` | `LBR-CLI-002` | 129 |
| Failed to read refs | `LBR-IO-001` | 128 |
| Corrupt stored branch/tag data | `LBR-REPO-002` | 128 |
