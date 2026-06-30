# `libra update-ref`

Safely update, create, or delete a branch ref with an optional
compare-and-swap — a focused subset of `git update-ref`. The ref read, the ref
write/delete, and the reflog entry all happen inside a single SQLite
transaction, so a failed compare-and-swap rolls everything back atomically.

## Synopsis

```
libra update-ref [-m <reason>] refs/heads/<branch> <newvalue> [<oldvalue>]
libra update-ref -d [-m <reason>] refs/heads/<branch> [<oldvalue>]
```

## Description

`update-ref` points `refs/heads/<branch>` at `<newvalue>` (creating the ref if
it does not exist), or deletes it with `-d`. The optional `<oldvalue>` is a
**compare-and-swap** guard:

- a full object id — the ref must currently point there, or the command fails;
- `0000…0000` (the all-zero id) — the ref must **not** already exist
  (create-only).

When `<oldvalue>` is omitted, the ref is created or overwritten unconditionally.

**Scope (v1):** only `refs/heads/<branch>` is supported — the branch-tip case
Libra's SQLite `reference` table models directly. `HEAD`, `refs/tags/*`,
`refs/remotes/*`, and arbitrary ref namespaces are rejected; use
[`symbolic-ref`](symbolic-ref.md) / [`switch`](switch.md) for `HEAD` and
[`tag`](tag.md) for tags.

Every successful update writes an `update-ref` reflog entry for the ref. The
`<oldvalue>` you pass for the compare-and-swap is **never** recorded in the
reflog message; only the actual before/after object ids are.

## Options

| Option | Description | Example |
|--------|-------------|---------|
| `-d`, `--delete` | Delete the ref instead of updating it. | `libra update-ref -d refs/heads/old` |
| `-m <reason>` | Reflog reason recorded with the update. | `libra update-ref -m "reset tip" refs/heads/main <oid>` |
| `<newvalue>` | The new object id (omit with `-d`). | |
| `<oldvalue>` | Expected current id for a compare-and-swap (`0{40}` = must not exist). | |
| `--json` / `--machine` | Structured output: `{ ref, old, new, deleted }`. | `libra --json update-ref refs/heads/main <oid>` |

A symbolic value (`ref:refs/heads/…`) and the null object id as `<newvalue>` are
rejected — use `symbolic-ref`, or `-d` to delete.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | The ref was updated, created, or deleted. |
| `128` | Not inside a repository, an unsupported/invalid ref, an invalid object id, a compare-and-swap mismatch, or deleting a ref that does not exist. |

## Examples

```bash
# Point a branch at a specific commit
libra update-ref refs/heads/main <oid>

# Compare-and-swap: only move main if it is still at <oldoid>
libra update-ref refs/heads/main <newoid> <oldoid>

# Create a branch only if it does not already exist
libra update-ref refs/heads/topic <oid> 0000000000000000000000000000000000000000

# Delete a branch ref, optionally guarded by its current value
libra update-ref -d refs/heads/old
libra update-ref -d refs/heads/old <oldoid>
```

## Comparison with Git

| Task | Libra | Git |
|------|-------|-----|
| Update a branch ref | `libra update-ref refs/heads/b <oid>` | `git update-ref refs/heads/b <oid>` |
| Compare-and-swap | `libra update-ref refs/heads/b <new> <old>` | `git update-ref refs/heads/b <new> <old>` |
| Delete a ref | `libra update-ref -d refs/heads/b` | `git update-ref -d refs/heads/b` |

Deferred (not exposed): non-`refs/heads/*` namespaces, `HEAD`, `--stdin` batch
updates, `--create-reflog`, and `--no-deref`.
