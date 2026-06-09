# libra stats

Show file statistics for the current working directory.

`libra stats` is a Libra-only extension (it has no `git` equivalent). It is a
read-only command that recursively scans the current working directory, counts
regular files, and groups them by file extension. The `.libra/` metadata
directory and the `target/` build directory are skipped.

## Synopsis

```bash
libra stats
```

## Description

- Walks the current working directory recursively.
- Counts every regular file and buckets it by extension. Files without an
  extension are reported under `no_extension`.
- Skips the `.libra/` and `target/` directories.
- Prints a human-readable summary by default, or a structured envelope with the
  global `--json` / `--machine` flags.

The command does not read the index or any commit; it reports the on-disk
working tree exactly as it is.

## Options

`libra stats` takes no command-specific options. It honors the global output
flags:

| Flag | Description |
|------|-------------|
| `--json[=<FORMAT>]` | Emit the result as JSON (`pretty`, `compact`, or `ndjson`). |
| `--machine` | Strict machine mode (`--json=ndjson --no-pager --color=never --quiet`). |
| `--quiet` | Suppress stdout. |

## Output

Human-readable:

```text
File statistics:
total: 42
no_extension: 3
md: 7
rs: 32
```

JSON (`--json`):

```json
{
  "total": 42,
  "extensions": {
    "md": 7,
    "no_extension": 3,
    "rs": 32
  }
}
```

## Examples

```bash
# Count working-tree files grouped by extension
libra stats

# Structured JSON output for agents/tooling
libra stats --json
```
