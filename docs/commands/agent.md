# `libra agent`

Manage external-agent capture for tools such as Claude Code and Gemini.

## Synopsis

```bash
libra agent status
libra agent enable [--agent <name>]...
libra agent disable [--agent <name>]...
libra agent session <subcommand>
libra agent checkpoint <subcommand>
libra agent clean [--all]
libra agent doctor
libra agent push [--remote <name>]
libra agent rpc <subcommand>
```

## Description

`libra agent` manages Libra's external-agent capture surface. It installs and
removes provider hooks, reports captured session/checkpoint state, exposes
read-only diagnostics, and can push `refs/libra/agent-traces` to a remote.

Stable installable agents currently include `claude-code` and `gemini`. Preview
adapters are discoverable in code but are skipped by install/uninstall until
their hook installation path is implemented.

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `status` | Report captured external-agent session status |
| `enable` | Enable one or more external agents and install hooks |
| `disable` | Disable one or more external agents and uninstall hooks |
| `session list` | List captured sessions |
| `session show <id>` | Show a captured session |
| `session stop <id>` | Stop a captured session when supported |
| `session resume <id>` | Resume a stopped session when supported |
| `session promote <id>` | Promote a captured session into Libra intent metadata |
| `session derive-tool-calls <id>` | Derive tool-call records from a captured session |
| `checkpoint list` | List captured checkpoints |
| `checkpoint show <id>` | Show checkpoint metadata |
| `checkpoint rewind <id>` | Inspect or apply a working-tree rewind for one checkpoint |
| `clean` | Clean up temporary checkpoints from stopped sessions |
| `doctor` | Diagnose hook installation and capture state |
| `push` | Push `refs/libra/agent-traces` to a remote |
| `rpc list` | List discovered `libra-agent-*` binaries on `PATH` |
| `rpc invoke` | Invoke one JSON-RPC method on a `libra-agent-*` binary |

## Common Options

| Flag | Subcommand | Description |
|------|------------|-------------|
| `--agent <name>` | `enable`, `disable` | Select agent names; omit to target all stable agents |
| `--all` | `clean` | Clean all stopped-session checkpoints instead of only the most recent |
| `--remote <name>` | `push` | Select the remote used for pushing agent trace refs |
| `--dry-run` | `checkpoint rewind` | Show the impact without modifying files; this is the default |
| `--apply` | `checkpoint rewind` | Restore the working tree for the selected checkpoint |

## JSON Output

Subcommands that support structured output use the global `--json` and
`--machine` envelope. For example:

```bash
libra --json agent status
libra --json agent checkpoint list
libra --json agent rpc list
```

## Notes

- The top-level `agent hooks` entry is hidden and intended for hook configs
  installed by `libra agent enable`; users normally do not call it directly.
- `checkpoint rewind --apply` restores working-tree files only; the agent's own
  transcript file is not rewritten.
- Hook and capture diagnostics are best-effort and are designed to report
  actionable installation state rather than silently ignoring missing providers.
