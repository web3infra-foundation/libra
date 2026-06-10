# `libra symbolic-ref`

Read or update Libra's symbolic `HEAD` reference.

## Synopsis

```bash
libra symbolic-ref [--short] [--quiet] [HEAD]
libra symbolic-ref [-m <reason>] HEAD refs/heads/<branch>
libra symbolic-ref HEAD refs/heads/<branch>
libra symbolic-ref -d HEAD
```

## Description

`libra symbolic-ref` is a Git-compatible plumbing command for inspecting or
changing the symbolic ref stored in `HEAD`. Libra currently supports the local
`HEAD` symbolic ref. Other symbolic refs are rejected because Libra stores refs
in SQLite rather than loose files under `.git/`.

When `HEAD` points at a branch, the read form prints `refs/heads/<branch>`.
When `HEAD` is detached, the command exits with an invalid-target error. With
`--quiet`, Libra suppresses the user-facing hint but still reports failure
through the normal structured error contract.

The update form is silent in human output when it succeeds.
When `-m <reason>` is supplied with the update form, Libra records that exact
message in the `HEAD` reflog entry. If the target branch is unborn and has no
commit yet, Libra updates `HEAD` but does not write a reflog row with a missing
object id.

`-d` / `--delete` is parsed for Git surface compatibility but is intentionally
rejected. Libra requires a root `HEAD` row in its SQLite reference storage; use
`switch` or `checkout` to change where `HEAD` points.

## Options

| Option | Description |
|--------|-------------|
| `--short` | Print only the branch name, for example `main` |
| `-q`, `--quiet` | Suppress extra guidance when `HEAD` is not symbolic |
| `-m <reason>` | Store `<reason>` as the `HEAD` reflog message when updating `HEAD` |
| `-d`, `--delete` | Rejected (`LBR-CONFLICT-002`, exit 128): Libra stores refs in SQLite and `HEAD` is its only symbolic ref, so there is nothing to delete. Use `libra switch <branch>` or `libra checkout <branch>` to repoint `HEAD` |
| `HEAD` | The symbolic ref to inspect or update. Omitted defaults to `HEAD` |
| `refs/heads/<branch>` | New symbolic target for `HEAD` |

## Examples

```bash
libra symbolic-ref HEAD
libra symbolic-ref --short HEAD
libra symbolic-ref HEAD refs/heads/main
libra symbolic-ref -m "manual move" HEAD refs/heads/main
libra --json symbolic-ref HEAD
```

## Structured Output

```json
{
  "ok": true,
  "command": "symbolic-ref",
  "data": {
    "name": "HEAD",
    "target": "refs/heads/main",
    "short": "main",
    "action": "read"
  }
}
```

For updates, `action` is `set`.

## Compatibility Notes

- Libra supports `HEAD` only.
- Update targets must be local branch refs under `refs/heads/`.
- The command can point `HEAD` at an unborn branch, matching Git's ability to
  store a symbolic branch target before the branch has a commit.
- `-d` / `--delete` is intentionally rejected instead of deleting Libra's root
  `HEAD` row.
- `--recurse` / `--no-recurse` are not exposed because Libra has no symbolic
  ref chains to traverse.
