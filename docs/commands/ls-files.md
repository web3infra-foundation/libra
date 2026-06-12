# libra ls-files

## Synopsis

```sh
libra ls-files [--cached] [--deleted] [--modified] [--stage|-s] [--others] [--exclude-standard]
```

## Description

Lists paths known to Libra's index, with basic filters for cached, deleted, modified, conflict-stage, and untracked files. This command reads Libra's `.libra/index` and working tree; it does not use a `.git/index` layout.

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

`ls-files` is `partial`. Deferred Git flags include pathspecs, `-z`, `--error-unmatch`, ignored-mode variants, explicit exclude sources, `--eol`, resolve-undo, killed/debug output, and sparse-checkout integration.

## Structured Output

`--json` and `--machine` return the standard Libra envelope. `data` is an array of entries with `path`, `hash`, `mode`, `stage`, and `status` fields. Fields that do not apply to untracked files are `null`.
