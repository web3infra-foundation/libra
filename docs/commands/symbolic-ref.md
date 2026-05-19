# `libra symbolic-ref`

Read or update Libra's symbolic `HEAD` reference.

## Synopsis

```bash
libra symbolic-ref [--short] [--quiet] [HEAD]
libra symbolic-ref HEAD refs/heads/<branch>
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

## Options

| Option | Description |
|--------|-------------|
| `--short` | Print only the branch name, for example `main` |
| `-q`, `--quiet` | Suppress extra guidance when `HEAD` is not symbolic |
| `HEAD` | The symbolic ref to inspect or update. Omitted defaults to `HEAD` |
| `refs/heads/<branch>` | New symbolic target for `HEAD` |

## Examples

```bash
libra symbolic-ref HEAD
libra symbolic-ref --short HEAD
libra symbolic-ref HEAD refs/heads/main
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
