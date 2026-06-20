# libra ls-files

Historical design for listing index entries.

> Status: unpublished. Top-level `libra ls-files` is not registered in the
> public CLI in the current release. Running it returns the standard
> unknown-command error (`LBR-CLI-001`). This page describes preserved design
> material for a future top-level command. The published LFS command remains
> `libra lfs ls-files`.

## Synopsis

```sh
libra ls-files [--cached] [--deleted] [--modified] [--stage|-s] [--others] [--exclude-standard]
```

## Description

The unpublished design lists paths known to Libra's index, with basic filters for cached, deleted, modified, conflict-stage, and untracked files. It reads Libra's `.libra/index` and working tree; it does not use a `.git/index` layout.

## Options

| Option | Description |
|---|---|
| `--cached` | Show cached index entries. This is the default when no state filter is provided. |
| `--deleted` | Show tracked paths whose working-tree file is missing. |
| `--modified` | Show tracked paths whose working-tree content hash differs from the index. |
| `--stage` | Include stage information, including conflict stages when present. |
| `-s` | Short stage-style output: `<mode> <object> <stage>\t<path>`. |
| `--others` | Show untracked working-tree files. |
| `--exclude-standard` | With `--others`, honor `.libraignore` rules. |

## Examples

```sh
libra ls-files
libra ls-files --modified
libra ls-files --deleted
libra ls-files --others --exclude-standard
libra --json ls-files --stage
```

## Compatibility

If this command is published in a future release, its intended compatibility tier is `partial`. Deferred Git flags include pathspecs, `-z`, `--error-unmatch`, ignored-mode variants, explicit exclude sources, `--eol`, resolve-undo, killed/debug output, and sparse-checkout integration.

## Structured Output

If this command is published in a future release, `--json` and `--machine` should return the standard Libra envelope. `data` is expected to be an array of entries with `path`, `hash`, `mode`, `stage`, and `status` fields. Fields that do not apply to untracked files are `null`.
