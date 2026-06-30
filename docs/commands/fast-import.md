# `libra fast-import`

Import a `git fast-import` stream into the repository — a focused subset of
`git fast-import`. The natural counterpart to [`fast-export`](fast-export.md).

## Synopsis

```
libra fast-import [--input <file>] [--max-count <n>] [--quiet]
```

## Description

`fast-import` reads a fast-import stream from stdin (or `--input <file>`) and
writes the objects and refs it describes. Supported directives:

- `blob` with `mark` / `data`;
- `commit <ref>` with `mark`, `author`, `committer`, `data` (message), `from`,
  `merge`, and the file operations `M <mode> <dataref> <path>`, `D <path>`, and
  `deleteall`;
- `reset <ref>` with an optional `from`;
- `checkpoint`, `done`;
- the lenient preamble `feature` / `option` / `progress` (accepted, ignored).

`tag`, `cat-blob`, `ls`, `get-mark`, note (`N`), and copy/rename (`C` / `R`) are
not yet supported and are rejected.

### Transaction model

Objects are written as they are parsed, but **ref updates are buffered** and
applied only at a `checkpoint`, at `done`, or at a clean end-of-stream. A stream
that is truncated mid-object fails before that flush, so branches are never left
half-updated; the orphaned objects are unreferenced and reclaimed by a later
`libra gc`. To recover after an interrupted import, run `libra fsck` then
`libra gc`.

### Safety and resource limits

- Total input is capped (default **1 GiB**, configurable via
  `fastimport.maxInputSize`).
- The number of **blobs and commits** created is capped (default **1,000,000**);
  raise it with `--max-count <n>`. (Trees are derived and written through the
  shared `write-tree` path and are not separately counted.)
- Refs must be under `refs/…`, well-formed, and never escape the repository.
- Object ids referenced literally must match the repository's hash length
  (SHA-1 / SHA-256); duplicate marks are rejected.

## Options

| Option | Description |
|--------|-------------|
| `--input <file>` | Read the stream from a file instead of stdin. |
| `--max-count <n>` | Raise the blob+commit-count ceiling for this import. |
| `--quiet` | Suppress the final summary line. |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | The stream imported successfully. |
| `128` | Not inside a repository, a malformed stream, a duplicate mark, an invalid/out-of-repo ref, a hash-format mismatch, a resource-limit overflow, or an IO error. |

## Examples

```bash
# Round-trip history through a stream
libra fast-export main | libra fast-import

# Import a saved stream
libra fast-import < repo.fastimport
libra fast-import --input repo.fastimport
```

## Comparison with Git

| Task | Libra | Git |
|------|-------|-----|
| Import a stream | `libra fast-import` | `git fast-import` |

Differences and deferred features: only branch refs (`refs/heads/*`) are
persisted (other namespaces are parsed but not yet written); `tag`, `cat-blob`,
`ls`, `get-mark`, notes, copy/rename, marks-file import/export (`--import-marks`
/ `--export-marks`), and true streaming of multi-gigabyte inputs are not yet
implemented.
