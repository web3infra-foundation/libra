# `libra bundle`

Create and inspect Git **v2 bundle** files — a single-file archive of repository
history that Git (or another Libra) can read. A focused subset of `git bundle`.

## Synopsis

```
libra bundle create <file> <rev>...
libra bundle verify <file>
libra bundle list-heads <file>
```

## Description

A bundle is a small text header followed by a pack:

```text
# v2 git bundle
<tip-oid> <ref-name>      (one line per included ref)
                          (blank line)
PACK……                    (a v2 pack of every reachable object)
```

- **`create <file> <rev>...`** — resolve each `<rev>` to a tip, collect every
  object reachable from those tips, and write them as a full (non-thin) bundle.
  Each `<rev>` becomes a head line (`<oid> refs/heads/<name>`). The file is
  written to a temporary path and renamed into place, so a failure never leaves
  a half-written bundle.
- **`verify <file>`** — check that the header is a valid `# v2 git bundle`, that
  the pack is present (`PACK` v2), and that any prerequisite objects already
  exist locally. Prints `<file> is okay` and the heads.
- **`list-heads <file>`** — print the `<oid> <ref>` head lines the bundle carries.

The pack is encoded with the repository's hash kind, so both SHA-1 and SHA-256
repositories produce correctly-sized object ids.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success (bundle written / valid / heads listed). |
| `1` | `verify` / `list-heads`: the bundle is invalid or unreadable, or a prerequisite is missing (matching `git bundle verify`). |
| `128` | Not inside a repository, or `create` hit a bad revision or write error. |

## Examples

```bash
libra bundle create repo.bundle main          # bundle the main branch
libra bundle create snapshot.bundle HEAD       # bundle the current branch
git clone repo.bundle restored                 # system Git can read it
libra bundle verify repo.bundle
libra bundle list-heads repo.bundle
```

## Comparison with Git

| Task | Libra | Git |
|------|-------|-----|
| Create | `libra bundle create <f> <rev>` | `git bundle create <f> <rev>` |
| Verify | `libra bundle verify <f>` | `git bundle verify <f>` |
| List heads | `libra bundle list-heads <f>` | `git bundle list-heads <f>` |

Differences and deferred features: only full bundles are written (no
prerequisite / thin / incremental `<rev>..<rev>` bundles yet); `unbundle` and
cloning **from** a bundle through `libra` are not implemented (use `git clone
<file>`); `verify` checks the header and pack magic rather than fully validating
the pack checksum (use `libra index-pack` / `libra fsck` for that).
