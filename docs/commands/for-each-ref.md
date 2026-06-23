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
| `%(refname:short)` | Short ref name (namespace prefix stripped), e.g. `main` |
| `%(refname:lstrip=N)` | Ref name with `N` leading path components removed (`N<0` keeps the last `|N|`) |
| `%(refname:rstrip=N)` | Ref name with `N` trailing path components removed (`N<0` keeps the first `|N|`) |
| `%(objectname)` | Object hash the ref points to |
| `%(objectname:short)` | Abbreviated object hash (7 characters) |
| `%(objectname:short=N)` | Abbreviated object hash to `N` characters (capped at the full length) |
| `%(objecttype)` | Object type: `commit`, `tag`, `tree`, or `blob` |
| `%(HEAD)` | `*` if the ref is the currently checked-out branch, otherwise a space |
| `%(upstream)` | The branch's upstream tracking ref (e.g. `refs/remotes/origin/main`); empty when none |
| `%(upstream:short)` | The upstream ref with the `refs/remotes/` prefix stripped (e.g. `origin/main`) |
| `%(subject)` | First line of the ref object's message (commit or annotated-tag message); empty for trees/blobs |
| `%(contents)` | Full message of the commit/annotated-tag object |
| `%(contents:subject)` | Same as `%(subject)` |
| `%(body)` / `%(contents:body)` | Message body — everything after the first blank line |
| `%(authorname)` | Commit author name (empty for non-commit refs such as annotated tags) |
| `%(authoremail)` | Commit author email, angle-bracketed (e.g. `<a@example.com>`); empty for non-commit refs |
| `%(committername)` | Commit committer name; empty for non-commit refs |
| `%(committeremail)` | Commit committer email, angle-bracketed; empty for non-commit refs |
| `%(taggername)` | Annotated-tag tagger name; empty for non-tag refs (lightweight tags and commits) |
| `%(taggeremail)` | Annotated-tag tagger email, angle-bracketed; empty for non-tag refs |
| `%(authordate)` | Commit author date in Git's default format; empty for non-commit refs |
| `%(committerdate)` | Commit committer date in Git's default format; empty for non-commit refs |
| `%(taggerdate)` | Annotated-tag tagger date in Git's default format; empty for non-tag refs |

Date atoms use Git's default format (`Day Mon DD HH:MM:SS YYYY +ZZZZ`) and, like
`libra log`, render in UTC (`+0000`) rather than the commit's original timezone.
The `:short`/`:iso`/`:relative` date modifiers are not yet supported.

## Options

| Option | Description |
|---|---|
| `--heads` | List local branch refs under `refs/heads/`. |
| `--tags` | List tag refs under `refs/tags/`. |
| `--remotes` | List remote-tracking refs under `refs/remotes/`. |
| `--all` | List all supported ref namespaces. This is the default when no namespace flag is given. |
| `--format=<format>` | Render simple atoms. Supported atoms: `%(refname)`, `%(refname:short)`, `%(refname:lstrip=N)`, `%(refname:rstrip=N)`, `%(objectname)`, `%(objectname:short)` (7-char), `%(objectname:short=N)`, `%(objecttype)`, `%(HEAD)`, `%(upstream)`, `%(upstream:short)`, `%(subject)`, `%(contents)`, `%(contents:subject)`, `%(body)`, `%(contents:body)`, `%(authorname)`, `%(authoremail)`, `%(committername)`, `%(committeremail)`, `%(taggername)`, `%(taggeremail)`, `%(authordate)`, `%(committerdate)`, `%(taggerdate)`. |
| `--sort=<key>` | Sort by `refname`, `objectname`, or `version:refname` (alias `v:refname`; orders embedded numbers numerically, so `v1.9` precedes `v1.10`). Prefix any key with `-` to reverse. |
| `--count=<n>` | Limit output to at most `n` refs after filtering and sorting. |
| `--points-at=<object>` | Keep refs that point at the object. Annotated tags also match their peeled target. |
| `--contains=<commit>` / `--no-contains=<commit>` | Keep (or exclude) refs whose tip has `<commit>` as an ancestor. |
| `--merged=<commit>` / `--no-merged=<commit>` | Keep (or exclude) refs whose tip is reachable from `<commit>` (already merged into it). |
| `--exclude=<pattern>` | Do not list refs matching `<pattern>` (repeatable; applied after the positional include patterns). |
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

Compatibility tier is `partial`. `--contains` / `--no-contains` are supported (filter refs whose tip has, or does not have, the given commit as an ancestor), as are `--merged` / `--no-merged` (filter refs whose tip is, or is not, reachable from the given commit) and `--exclude` (drop refs matching the given pattern, applied after the positional include patterns). Supported sort keys are `refname`, `objectname`, and `version:refname` (each reversible with a `-` prefix). Deferred Git features include the full atom language, the remaining sort keys (e.g. `*objectname`, date keys), and shell/perl/python/tcl quoting modes. Git flat-file ref storage parity is intentionally not applicable to Libra.

## Structured Output

`--json` and `--machine` return the standard Libra envelope. `data` is an array of entries with `refname`, `objectname`, and `objecttype` fields.
