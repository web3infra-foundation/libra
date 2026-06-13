# libra for-each-ref

Historical design for enumerating references.

> Status: unpublished. `libra for-each-ref` is not registered in the public CLI
> in the current release. Running it returns the standard unknown-command error
> (`LBR-CLI-001`). The interface below documents preserved implementation notes,
> not a user-visible command contract.

## Synopsis

```sh
libra for-each-ref [--heads] [--tags] [--remotes] [--all] [--format=<format>] [--sort=<key>] [--count=<n>] [<pattern>...]
```

## Description

The unpublished design enumerates refs stored in Libra's SQLite-backed ref model. It covers local branches, remote-tracking branches, and tags. It does not read `.git/refs` or `packed-refs`.

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
| `<pattern>...` | Keep refs whose full name matches, contains, or ends with the pattern. |

## Examples

```sh
libra for-each-ref
libra for-each-ref --heads
libra for-each-ref --tags --format='%(refname) %(objectname)'
libra for-each-ref --sort=-refname --count=5
libra --json for-each-ref --remotes
```

## Compatibility

If this command is published in a future release, its intended compatibility tier is `partial`. Deferred Git features include the full atom language, full sort keys, `--contains` / `--no-contains`, `--merged` / `--no-merged`, `--points-at`, and shell/perl/python/tcl quoting modes. Git flat-file ref storage parity is intentionally not applicable to Libra.

## Structured Output

If this command is published in a future release, `--json` and `--machine` should return the standard Libra envelope. `data` is expected to be an array of entries with `refname`, `objectname`, and `objecttype` fields.
