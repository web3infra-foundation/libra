# `libra rev-parse`

Parse revision names and print normalized commit IDs, symbolic refs, or repository paths.

## Synopsis

```bash
libra rev-parse [OPTIONS] [SPEC]
```

## Description

`libra rev-parse` resolves revision-like input into script-friendly values:

- the full commit ID (default)
- a short commit ID with `--short`
- a symbolic branch name with `--abbrev-ref`
- full symbolic ref names with `--symbolic-full-name`
- repository path/state values such as `--git-dir`, `--show-prefix`, and `--is-inside-work-tree`
- range endpoint streams for `A..B`, `A...B`, and `^A`

It also supports `--show-toplevel` to print the absolute repository root for a working tree, and `--verify` to assert that an argument resolves to exactly one object. When no `<SPEC>` is provided, the command defaults to `HEAD` (or `--default <SPEC>` when supplied).

## Options

| Flag | Description |
|------|-------------|
| `--short` | Print a non-ambiguous abbreviated object ID. |
| `--abbrev-ref` | Print the symbolic branch name instead of a commit hash. |
| `--verify` | Require the argument to resolve to a single object and print it; otherwise fail. May be combined with `--short`. Conflicts with `--abbrev-ref`, path/state flags, and shell-quote modes. |
| `--default <SPEC>` | Revision to use when no positional `<SPEC>` is given. |
| `--show-toplevel` | Print the absolute path to the top-level working tree. |
| `--git-dir` | Print the Libra storage directory (`.libra`, not `.git`). |
| `--show-prefix` | Print the current directory relative to the worktree root, using `/` and a trailing `/` when non-empty. |
| `--show-cdup` | Print the `../` path needed to return from the current directory to the worktree root. |
| `--is-inside-git-dir` | Print `true` when the current directory is inside `.libra`, otherwise `false`. |
| `--is-inside-work-tree` | Print `true` in the worktree and `false` from inside `.libra`. Outside a repository this is a fatal repo-not-found error. |
| `--is-bare-repository` | Print the parsed `core.bare` value. |
| `--sq` | Resolve each revision and shell-quote the resolved outputs on one line. |
| `--sq-quote` | Shell-quote positional arguments literally without requiring a repository. |
| `--symbolic` | Prefer symbolic input spelling where Libra can preserve it. |
| `--symbolic-full-name` | Resolve branch, remote-tracking, and tag names to full `refs/...` names. |
| `<SPEC>...` | Revisions or range expressions to resolve. Defaults to `HEAD` when omitted outside `--verify` and `--sq-quote`. |

### `--verify` exit codes

`--verify` mirrors Git's plumbing contract:

- Success: prints the resolved hash, exits 0.
- Failure (invalid ref, unborn HEAD, or no revision): prints `fatal: Needed a single revision` to stderr and exits **128**.
- Failure under the global `--quiet`/`-q`: prints nothing and exits **1** (matching `git rev-parse --verify -q`).

> Note: without `--verify`, an invalid spec exits **129** (`LBR-CLI-003`) — an intentional difference from Git's 128 on that path, kept consistent with Libra's invalid-target exit-code model.

## Common Commands

```bash
libra rev-parse
libra rev-parse HEAD~1
libra rev-parse --short HEAD
libra rev-parse --abbrev-ref HEAD
libra rev-parse --verify HEAD
libra rev-parse --verify --default HEAD
libra rev-parse --show-toplevel
libra rev-parse --show-prefix
libra rev-parse --git-dir
libra rev-parse HEAD~1..HEAD
libra rev-parse --sq HEAD main
libra rev-parse --sq-quote -x "a b"
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

With `--show-prefix` from `src/command`:

```text
src/command/
```

With a two-dot range:

```text
<HEAD hash>
^<HEAD~1 hash>
```

With `--sq-quote`:

```text
 '-x' 'a b'
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

`mode` is one of `resolve`, `short`, `abbrev_ref`, `verify`, `show_toplevel`, `git_dir`, `show_prefix`, `show_cdup`, `is_inside_git_dir`, `is_inside_work_tree`, `is_bare_repository`, `range`, `symbolic`, or `symbolic_full_name`.

Range and multi-spec JSON output keeps `value` as the newline-joined text output and adds ordered `values`:

```json
{
  "ok": true,
  "command": "rev-parse",
  "data": {
    "mode": "range",
    "input": "HEAD~1..HEAD",
    "value": "<HEAD hash>\n^<HEAD~1 hash>",
    "values": ["<HEAD hash>", "^<HEAD~1 hash>"]
  }
}
```

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Resolve full commit ID | `rev-parse <spec>` | `git rev-parse <spec>` | `jj log -r <rev> --no-graph -T commit_id` |
| Abbreviated commit ID | `--short` | `--short` | `jj log -r <rev> -T change_id.short()` |
| Symbolic branch name | `--abbrev-ref` | `--abbrev-ref` | N/A |
| Verify single object | `--verify` (exit 128, or 1 under `-q`) | `--verify` | N/A |
| Default revision | `--default <SPEC>` | `--default` | N/A |
| Work tree root | `--show-toplevel` | `--show-toplevel` | `jj root` |
| Path/state queries | `--git-dir`, `--show-prefix`, `--show-cdup`, `--is-inside-git-dir`, `--is-inside-work-tree`, `--is-bare-repository` | All supported | `jj root` (partial) |
| Shell quoting / ranges | `--sq`, `--sq-quote`, `A..B`, `A...B`, `^A` | All supported | revsets |
| JSON output | `--json` | No | No |

## Intentional Differences

- `--git-dir` returns Libra's `.libra` storage directory rather than `.git`.
- Without `--verify`, an invalid target returns Libra's invalid-target exit code 129. Git returns 128 for some comparable cases.
- `--verify` is a single-object mode. Libra rejects combinations with path/state flags instead of streaming mixed outputs.
- `--sq-quote -- -x` treats the leading `--` as clap's option terminator, so the terminator is not included in the quoted output. Use `--sq-quote -x "a b"` for leading hyphen literals.
- `--symbolic-full-name` covers local branches, remote-tracking branches, and tags. More obscure Git symbolic forms are partial.

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid target ref (no `--verify`) | `LBR-CLI-003` | 129 |
| `--verify` failure (no `--quiet`) | `LBR-REPO-003` | 128 |
| `--verify` failure (with `--quiet`) | (silent) | 1 |
| Invalid work tree state | `LBR-REPO-003` | 128 |
| Failed to read repository metadata | `LBR-IO-001` | 128 |
| Corrupt stored refs/config | `LBR-REPO-002` | 128 |
| Shell-quote mode with `--json`/`--machine` | `LBR-CLI-002` | 129 |
