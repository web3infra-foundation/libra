# `libra fast-export`

Emit the history reachable from a revision as a `git fast-import` stream — a
focused subset of `git fast-export`. Read-only: it never writes objects or refs.

## Synopsis

```
libra fast-export [<rev>]
```

## Description

`fast-export` walks the commits reachable from `<rev>` (default `HEAD`),
oldest-first, and writes a fast-import stream to stdout:

- each blob is emitted once with a `mark`;
- each commit is emitted with its `author`/`committer`/`data` (message),
  `from`/`merge` links to its parents, then `deleteall` followed by an `M`
  line per file — the commit's whole tree is reconstructed rather than diffed
  against its parent. The stream is larger than Git's diff-based output but
  byte-for-byte equivalent.

The commits are emitted under the branch ref that `<rev>` resolves to
(`refs/heads/<branch>` for `HEAD` / a branch name).

## Options

| Option | Description | Example |
|--------|-------------|---------|
| `<rev>` | Revision whose reachable commits to export (default `HEAD`). | `libra fast-export main` |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | The stream was written. |
| `128` | Not inside a repository, an unresolvable revision, or an object/IO error. |

## Examples

```bash
# Save the current branch as a stream
libra fast-export > repo.fastimport

# Pipe into another importer
libra fast-export main | git fast-import --quiet   # in another repo
```

## Comparison with Git

| Task | Libra | Git |
|------|-------|-----|
| Export history | `libra fast-export <rev>` | `git fast-export <rev>` |

Differences and deferred features: the output reconstructs each commit's full
tree (`deleteall` + `M` list) instead of a parent diff, so it is larger;
exporting multiple refs at once, annotated/signed tags, `--export-marks` /
`--import-marks`, and blob/path filtering are not yet supported.
