# `libra for-each-ref`

List local refs with filtering and custom formatting.

> Status: public CLI with partial Git compatibility. The command enumerates references stored in Libra's SQLite-backed ref model. It covers local branches, remote-tracking branches, tags, and `--points-at` filtering. It does not read `.git/refs` or `packed-refs`.

## Synopsis

```sh
libra for-each-ref [--heads] [--tags] [--remotes] [--all] [--format=<format>] [--sort=<key>] [--count=<n>] [--points-at=<object>] [<pattern>...]
```

## Description

`libra for-each-ref` enumerates refs stored in the repository (branches, tags, and remote-tracking refs) and prints each ref's object hash and name. Use `--heads`, `--tags`, or `--remotes` to restrict output to one namespace; the default is `--all`.

Positional `<pattern>` arguments act as substring filters on the fully-qualified ref name (e.g., `refs/heads/main`). Only refs whose name matches, contains, or ends with at least one pattern are included.

Use `--points-at <object>` to keep refs that point at the resolved object. Annotated tags match both their tag object and their peeled target commit, matching Git's common `for-each-ref --points-at HEAD` behavior.

The `--format` option accepts a simple atom language. Supported atoms:

| Atom | Value |
|---|---|
| `%(refname)` | Full ref name, e.g. `refs/heads/main` |
| `%(objectname)` | Object hash the ref points to |
| `%(objecttype)` | Object type: `commit`, `tag`, `tree`, or `blob` |

## Options

| Option | Description |
|---|---|
| `--heads` | List local branch refs under `refs/heads/`. |
| `--tags` | List tag refs under `refs/tags/`. |
| `--remotes` | List remote-tracking refs under `refs/remotes/`. |
| `--all` | List all supported ref namespaces. This is the default when no namespace flag is given. |
| `--format=<format>` | Render simple atoms. Supported atoms: `%(refname)`, `%(objectname)`, `%(objecttype)`. |
| `--sort=<key>` | Sort by `refname`, `-refname`, `objectname`, or `-objectname`. |
| `--count=<n>` | Limit output to at most `n` refs after filtering and sorting. |
| `--points-at=<object>` | Keep refs that point at the object. Annotated tags also match their peeled target. |
| `<pattern>...` | Keep refs whose full name matches, contains, or ends with the pattern. |

## Examples

```sh
libra for-each-ref
libra for-each-ref --heads
libra for-each-ref --tags --format='%(refname) %(objectname)'
libra for-each-ref --points-at HEAD --format='%(refname) %(objecttype)'
libra for-each-ref --sort=-refname --count=5
libra --json for-each-ref --remotes
```

## Compatibility

Compatibility tier is `partial`. Deferred Git features include the full atom language, full sort keys, `--contains` / `--no-contains`, `--merged` / `--no-merged`, and shell/perl/python/tcl quoting modes. Git flat-file ref storage parity is intentionally not applicable to Libra.

## Structured Output

`--json` and `--machine` return the standard Libra envelope. `data` is an array of entries with `refname`, `objectname`, and `objecttype` fields.
