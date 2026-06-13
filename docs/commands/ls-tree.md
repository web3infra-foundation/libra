# `libra ls-tree`

Historical design for listing the contents of a tree object.

> Status: unpublished. `libra ls-tree` is not registered in the public CLI in
> the current release. Running it returns the standard unknown-command error
> (`LBR-CLI-001`). The implementation notes below describe preserved design
> material, not a user-visible command contract.

## Synopsis

```bash
libra ls-tree [OPTIONS] <TREE-ISH> [PATH...]
```

## Description

The unpublished design resolves `<TREE-ISH>` to a commit root tree or to a tree object
hash, then prints entries from that tree. It is read-only: it does not update
refs, the index, the worktree, or object storage.

The first compatibility slice supports ordinary repository-relative path prefix
filters. Full Git pathspec matching, `REV:path` tree-ish syntax, `--format`,
`--full-name`, and `--full-tree` are deferred.

## Options

| Flag | Description |
|------|-------------|
| `-r`, `--recursive` | Recurse into subtrees |
| `-t` | Show tree entries while recursing |
| `-d` | Show matching tree entries themselves rather than their children |
| `-l`, `--long` | Show blob sizes; tree and commit entries use `-` |
| `-z` | Terminate records with NUL instead of newline |
| `--name-only` | Print only entry paths |
| `--name-status` | Git-compatible alias that prints only entry paths |
| `--object-only` | Print only object IDs |
| `--abbrev[=<N>]` | Abbreviate object IDs to `N` characters, or 7 when `N` is omitted |
| `<TREE-ISH>` | Commit, branch, tag, `HEAD`, or tree object hash |
| `[PATH...]` | Optional repository-relative path prefix filters |

## Examples

```bash
libra ls-tree HEAD
libra ls-tree -r HEAD src
libra ls-tree -l HEAD README.md
libra ls-tree --name-only HEAD src
libra ls-tree --object-only --abbrev HEAD
libra ls-tree -z HEAD
libra --json ls-tree HEAD
```

## Human Output

Default output matches Git's common shape:

```text
100644 blob 4f3c2d1a7b8c9d0e1234567890abcdef12345678	README.md
040000 tree 5a6b7c8d9e0f1234567890abcdef1234567890	src
```

With `-l`, blob entries include their decoded object size:

```text
100644 blob 4f3c2d1a7b8c9d0e1234567890abcdef12345678      128	README.md
040000 tree 5a6b7c8d9e0f1234567890abcdef1234567890        -	src
```

## Structured Output

If this command is published in a future release, `--json` should use the standard command envelope. With `-l`, blob
entries include the `size` field:

```json
{
  "ok": true,
  "command": "ls-tree",
  "data": {
    "treeish": "HEAD",
    "root_tree": "5a6b7c8d9e0f1234567890abcdef1234567890",
    "recursive": false,
    "entries": [
      {
        "mode": "100644",
        "object_type": "blob",
        "object": "4f3c2d1a7b8c9d0e1234567890abcdef12345678",
        "path": "README.md",
        "size": 128
      }
    ]
  }
}
```

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Commit/tree listing | Supported | Supported | Use file/revset commands |
| Recursive listing | `-r` / `--recursive` | `-r` | Different model |
| Tree entries while recursive | `-t` | `-t` | Different model |
| Path filters | Prefix filters only | Full pathspec | Revset/file patterns |
| Custom formatting | Deferred | `--format` | Different model |
| JSON output | `--json` | No | No |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid or missing tree-ish | `LBR-CLI-003` | 129 |
| Unsupported `REV:path` syntax | `LBR-UNSUPPORTED-001` | 128 |
| Failed to read objects | `LBR-IO-001` | 128 |
| Corrupt stored refs/objects | `LBR-REPO-002` | 128 |
