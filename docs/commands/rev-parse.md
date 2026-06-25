# `libra rev-parse`

Parse revision names and print normalized commit IDs, symbolic refs, or repository paths.

## Synopsis

```bash
libra rev-parse [OPTIONS] [SPEC]
```

## Description

`libra rev-parse` resolves a revision-like input into one of three forms:

- the full commit ID (default)
- a short commit ID with `--short`
- a symbolic branch name with `--abbrev-ref`

It also supports `--show-toplevel` to print the absolute repository root for a working tree. When no `<SPEC>` is provided, the command defaults to `HEAD`.

## Options

| Flag | Description |
|------|-------------|
| `--verify` | Assert that `<SPEC>` resolves to exactly one object; print it, or exit 128 on failure (silent exit 1 under the global `--quiet` / `-q`). |
| `--short` | Print a non-ambiguous abbreviated object ID. |
| `--sq` | Shell-quote the resolved object name (single-quoted) for safe shell consumption. Only affects the resolved-revision output, not query modes like `--show-toplevel`. |
| `--abbrev-ref` | Print the symbolic branch name instead of a commit hash. |
| `--symbolic-full-name` | Resolve the spec to its full ref name (`refs/heads/<branch>`, `refs/tags/<tag>`, `refs/remotes/<remote>/<branch>`, or `HEAD` when detached). A valid object that is not a ref prints nothing (exit 0); an unresolvable name fails with exit 128. |
| `--show-toplevel` | Print the absolute path to the top-level working tree. |
| `--is-inside-git-dir` | Print `true` when the current directory is inside the `.libra` directory (Libra's `$GIT_DIR` equivalent), `false` otherwise. |
| `--git-dir` | Print the path to the `.libra` directory (Libra's `$GIT_DIR`). In Libra this is always absolute. |
| `--absolute-git-dir` | Like `--git-dir`, but always the canonicalized absolute path. (In Libra `--git-dir` is already absolute, so the two coincide.) |
| `<SPEC>` | Revision to resolve. Defaults to `HEAD` when omitted. |

## Common Commands

```bash
libra rev-parse
libra rev-parse HEAD~1
libra rev-parse --short HEAD
libra rev-parse --abbrev-ref HEAD
libra rev-parse --show-toplevel
libra rev-parse --is-inside-git-dir
libra rev-parse --absolute-git-dir
libra --json rev-parse --short HEAD
```

## Human Output

Default output is a single line containing the resolved value.

```text
abc1234def5678901234567890abcdef12345678
```

With `--short`:

```text
abc1234
```

With `--abbrev-ref`:

```text
main
```

With `--show-toplevel`:

```text
/home/alice/project
```

## Structured Output

```json
{
  "ok": true,
  "command": "rev-parse",
  "data": {
    "mode": "short",
    "input": "HEAD",
    "value": "abc1234"
  }
}
```

`mode` is one of `resolve`, `short`, `abbrev_ref`, `symbolic_full_name`, `show_toplevel`, `show_prefix`, `show_cdup`, `is_inside_work_tree`, `is_inside_git_dir`, `is_bare_repository`, `git_dir`, or `absolute_git_dir`.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Resolve full commit ID | `rev-parse <spec>` | `git rev-parse <spec>` | `jj log -r <rev> --no-graph -T commit_id` |
| Abbreviated commit ID | `--short` | `--short` | `jj log -r <rev> -T change_id.short()` |
| Symbolic branch name | `--abbrev-ref` | `--abbrev-ref` | N/A |
| Full ref name | `--symbolic-full-name` | `--symbolic-full-name` | N/A |
| Shell-quoted output | `--sq` | `--sq` | N/A |
| Work tree root | `--show-toplevel` | `--show-toplevel` | `jj root` |
| JSON output | `--json` | No | No |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid target ref | `LBR-CLI-003` | 129 |
| Invalid work tree state | `LBR-REPO-003` | 128 |
| Failed to read repository metadata | `LBR-IO-001` | 128 |
| Corrupt stored refs/config | `LBR-REPO-002` | 128 |
