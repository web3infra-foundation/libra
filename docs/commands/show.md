# `libra show`

Show a commit, tag, tree, blob, or the blob referenced by `REV:path`.

## Synopsis

```
libra show [OPTIONS] [<OBJECT>] [-- <PATHS>...]
```

## Description

`libra show` resolves a single object reference and renders its contents. The
default target is `HEAD`. It understands commit-ish references (`HEAD~2`,
branch names, tag names), raw SHA-1 hashes, and the `REV:path` syntax for
extracting a specific blob from a tree at a given revision.

For commits the output includes the header (author, committer, date, message)
followed by a unified diff (the "patch"). Flags such as `--no-patch`, `--stat`,
and `--name-only` control how much diff context is shown. For annotated tags the
tagger metadata and message are printed, followed by the target object. Trees
list their entries and blobs print their text content (or a binary summary).

## Options

| Flag | Short | Description |
|------|-------|-------------|
| `<OBJECT>` | | Object name (commit, tag, tree, blob) or `<object>:<path>`. Defaults to `HEAD`. |
| `--no-patch` | `-s` | Skip patch output and only show object metadata. |
| `--oneline` | | Shorthand for `--pretty=oneline` -- prints hash and subject on one line. |
| `--name-only` | | Show only changed file names (no diff hunks). |
| `--stat` | | Show diff statistics (insertions / deletions per file). |
| `<PATHS>...` | | Limit output to matching paths (pathspec filter for commit diffs). |

### Examples

```bash
# Show the latest commit with full patch
libra show HEAD

# Show only metadata (no diff) for a tag
libra show --no-patch v1.0.0

# Show a specific file from a revision
libra show HEAD:src/main.rs

# One-line summary of a commit
libra show --oneline abc1234

# Diff statistics only
libra show --stat HEAD~1

# Limit diff to a subdirectory
libra show HEAD -- src/command/
```

## Common Commands

```bash
libra show                          # show HEAD commit and patch
libra show HEAD~3                   # show an ancestor commit
libra show -s v2.0.0                # metadata only for a tag
libra show HEAD:Cargo.toml          # print a file at HEAD
libra show --name-only HEAD         # list changed files
libra show --stat HEAD              # diff statistics
libra --json show HEAD              # structured JSON output
```

## Human Output

Human mode preserves the existing presentation:

- Commit: header plus optional patch / stat / name-only output
- Annotated tag: tag metadata followed by the target object
- Tree: list of tree entries
- Blob: text content or a binary summary
- `--quiet`: validates the object reference but suppresses human output

## Structured Output (JSON examples)

`data.type` determines the schema. Possible values: `commit`, `tag`, `tree`,
`blob`.

### Commit

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "commit",
    "hash": "abc1234def5678901234567890abcdef12345678",
    "short_hash": "abc1234",
    "author_name": "Alice",
    "author_email": "alice@example.com",
    "author_date": "2026-04-01T10:00:00+00:00",
    "committer_name": "Alice",
    "committer_email": "alice@example.com",
    "committer_date": "2026-04-01T10:00:00+00:00",
    "subject": "feat: add new feature",
    "body": "",
    "parents": ["def456..."],
    "refs": ["HEAD -> main"],
    "files": [
      { "path": "tracked.txt", "status": "added" }
    ]
  }
}
```

### Tag

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "tag",
    "tag_name": "v1.0.0",
    "tagger_name": "Alice",
    "tagger_email": "alice@example.com",
    "tagger_date": "2026-04-01T10:00:00+00:00",
    "message": "Release v1.0.0",
    "target_hash": "abc1234def5678901234567890abcdef12345678",
    "target_type": "commit"
  }
}
```

### Tree

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "tree",
    "entries": [
      { "mode": "100644", "object_type": "blob", "hash": "abc123...", "name": "README.md" },
      { "mode": "040000", "object_type": "tree", "hash": "def456...", "name": "src" }
    ]
  }
}
```

### Blob

```json
{
  "ok": true,
  "command": "show",
  "data": {
    "type": "blob",
    "hash": "abc123...",
    "size": 1024,
    "is_binary": false,
    "content": "fn main() { ... }"
  }
}
```

Notes:

- Commit JSON `refs` are best-effort decoration metadata; unrelated branch/tag rows no longer block `show`
- Human `--quiet` still validates the target object but suppresses stdout
- Commit patch / stat paths stay strict: corrupt historical blobs fail with `LBR-REPO-002` instead of falling back to working tree contents

## Design Rationale

### Why support `REV:path` syntax?

The `REV:path` notation (e.g., `HEAD:src/main.rs`) is one of the most useful
idioms in Git because it lets users and tools retrieve any file at any point in
history without checking out an entire commit. For AI agents this is especially
valuable: an agent can read specific files at specific revisions to compare
implementations across branches or time, without mutating the working tree.
Libra preserves this syntax for full Git compatibility and because it maps
naturally to the internal tree-walk operation that Libra already performs.

### Why no `--format`?

Git's `--format` / `--pretty=format:` machinery is powerful but complex, with
dozens of `%`-placeholders and conditional formatting. Libra instead provides
structured JSON output (`--json`) which gives programmatic consumers every field
in a well-typed schema. Human users get a sensible default presentation. This
avoids the maintenance burden of a format mini-language while giving agents a
strictly better interface (typed JSON fields vs. string parsing).

### Why type-aware JSON schema?

The `data.type` discriminator (`commit`, `tag`, `tree`, `blob`) means that
JSON consumers can switch on the type and access only the fields that exist for
that object kind. This is more ergonomic than a flat schema with many nullable
fields, and it mirrors the object model of Git itself. Each variant carries
exactly the fields that make sense (e.g., `tagger_name` appears only in tags,
`parents` only in commits), which eliminates an entire class of "field is null
but I expected it" bugs in agent tooling.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Default target | `HEAD` | `HEAD` | N/A (`jj show` removed; use `jj log -r @`) |
| `REV:path` syntax | Yes | Yes | No (use `jj file show -r REV path`) |
| `--no-patch` / `-s` | Yes | Yes | N/A |
| `--oneline` | Yes | Yes | N/A (use `jj log --template`) |
| `--name-only` | Yes | Yes | N/A |
| `--stat` | Yes | Yes | N/A (`jj diff --stat -r REV`) |
| `--format` / `--pretty=format:` | No (use `--json`) | Yes | No (use templates) |
| `--quiet` | Yes (validates only) | No | N/A |
| JSON output | `--json` with typed schema | No | No |
| Pathspec filter | Yes (trailing `<PATHS>...`) | Yes | No (use `jj diff --from/--to`) |
| Tag-aware display | Auto-detects annotated tags | Auto-detects annotated tags | No tag objects |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Outside a repository | `LBR-REPO-001` | 128 |
| Invalid revision or missing path | `LBR-CLI-003` | 129 |
| Failed to read the object | `LBR-REPO-002` | 128 |
