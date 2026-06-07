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

It also supports `--show-toplevel` to print the absolute repository root for a working tree, and `--verify` to assert that an argument resolves to exactly one object. When no `<SPEC>` is provided, the command defaults to `HEAD` (or `--default <SPEC>` when supplied).

## Options

| Flag | Description |
|------|-------------|
| `--short` | Print a non-ambiguous abbreviated object ID. |
| `--abbrev-ref` | Print the symbolic branch name instead of a commit hash. |
| `--verify` | Require the argument to resolve to a single object and print it; otherwise fail. May be combined with `--short`. Conflicts with `--abbrev-ref`/`--show-toplevel`. |
| `--default <SPEC>` | Revision to use when no positional `<SPEC>` is given. |
| `--show-toplevel` | Print the absolute path to the top-level working tree. |
| `<SPEC>` | Revision to resolve. Defaults to `HEAD` when omitted. |

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

`mode` is one of `resolve`, `short`, `abbrev_ref`, `verify`, or `show_toplevel`.

## Parameter Comparison: Libra vs Git vs jj

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Resolve full commit ID | `rev-parse <spec>` | `git rev-parse <spec>` | `jj log -r <rev> --no-graph -T commit_id` |
| Abbreviated commit ID | `--short` | `--short` | `jj log -r <rev> -T change_id.short()` |
| Symbolic branch name | `--abbrev-ref` | `--abbrev-ref` | N/A |
| Verify single object | `--verify` (exit 128, or 1 under `-q`) | `--verify` | N/A |
| Default revision | `--default <SPEC>` | `--default` | N/A |
| Work tree root | `--show-toplevel` | `--show-toplevel` | `jj root` |
| Path/state queries | Not implemented (deferred): `--git-dir`, `--show-prefix`, `--show-cdup`, `--is-*` | All supported | `jj root` (partial) |
| Shell quoting / ranges | Not implemented (deferred): `--sq`, `--sq-quote`, `A..B`, `A...B` | All supported | revsets |
| JSON output | `--json` | No | No |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Invalid target ref (no `--verify`) | `LBR-CLI-003` | 129 |
| `--verify` failure (no `--quiet`) | `LBR-REPO-003` | 128 |
| `--verify` failure (with `--quiet`) | (silent) | 1 |
| Invalid work tree state | `LBR-REPO-003` | 128 |
| Failed to read repository metadata | `LBR-IO-001` | 128 |
| Corrupt stored refs/config | `LBR-REPO-002` | 128 |
