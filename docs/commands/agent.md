# `libra agent`

Manage external-agent capture for Claude Code, Gemini, Cursor, Codex,
GitHub Copilot CLI, Factory AI Droid, and OpenCode.

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

All seven external agents are stable and hook-installable: `claude-code`,
`gemini`, `cursor`, `codex`, `copilot`, `factory-ai`, and `opencode`. Running
`libra agent enable` with no `--agent` flags installs hooks for every one.
Each agent's hooks are written into that agent's own config so its lifecycle
events forward to `libra agent hooks <agent> <event>`:

| Agent | Hook config file | Format |
|-------|------------------|--------|
| `claude-code` | `.claude/settings.json` | matcher groups |
| `gemini` | `~/.gemini/settings.json` | shell hooks |
| `cursor` | `.cursor/hooks.json` | per-event command arrays |
| `codex` | `.codex/hooks.json` | matcher groups |
| `copilot` | `.github/hooks/libra.json` | per-event command arrays |
| `factory-ai` | `.factory/settings.json` | matcher groups |
| `opencode` | `.opencode/plugins/libra.ts` | generated TypeScript plugin |

Installs are idempotent and preserve any non-Libra hooks already present;
`disable` removes only Libra-managed entries. Every installed command is
suffixed with `|| true` so a Libra hiccup never breaks the host agent.

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `status` | Report captured external-agent session status |
| `enable` | Enable one or more external agents and install hooks |
| `disable` | Disable one or more external agents and uninstall hooks |
| `session list` | List captured sessions |
| `session show <id>` | Show a captured session |
| `session stop <id>` | Mark a captured session as stopped |
| `session resume <id>` | Mark a stopped captured session active again |
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
| `--extract-transcript <path>` | `session show` | Copy the captured transcript path from session metadata to a local file |
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

## Examples

```bash
# Show captured-session counts and recent checkpoint summary
libra agent status

# Enable Claude Code capture and install its hooks
libra agent enable --agent claude

# Enable every stable external agent at once
libra agent enable

# Disable Claude Code capture and uninstall its hooks
libra agent disable --agent claude

# List captured sessions
libra agent session list

# Show a session and copy its captured transcript
libra agent session show <session-id> --extract-transcript /tmp/session.jsonl

# Stop a captured session
libra agent session stop <session-id>

# Resume a stopped captured session
libra agent session resume <session-id>

# List captured checkpoints
libra agent checkpoint list

# Show a single checkpoint by id
libra agent checkpoint show <id>

# Preview (default) or --apply a working-tree rewind for a checkpoint
libra agent checkpoint rewind <id>

# Drop temporary checkpoints from the most recent stopped session
libra agent clean

# Drop temporary checkpoints from every stopped session
libra agent clean --all

# Diagnose hook installation and capture state
libra agent doctor

# Push refs/libra/agent-traces to the default remote
libra agent push

# Push refs/libra/agent-traces to a named remote
libra agent push --remote origin

# Discover libra-agent-<name> RPC binaries on PATH
libra agent rpc list

# Invoke a single JSON-RPC method on a libra-agent-<slug> binary
libra agent rpc invoke <slug> <method> --params '<json>'

# Structured JSON envelope for agents
libra agent --json status
```

The same banner is rendered by `libra agent --help` so the doc and the
CLI surface stay in sync (cross-cutting `--help` EXAMPLES rollout, see
`docs/improvement/README.md` item B).

## Notes

- The top-level `agent hooks` entry is hidden and intended for hook configs
  installed by `libra agent enable`; users normally do not call it directly.
- `checkpoint rewind --apply` restores working-tree files to the checkpoint's
  parent commit. For agents with a `TranscriptTruncator` (currently Claude Code
  and Gemini) it also truncates the agent's local transcript back to the
  checkpoint boundary; for the remaining agents the working tree is restored but
  the transcript is left untouched and a notice is printed.
- Captured transcripts are redacted before they are written to
  `refs/libra/agent-traces`. The redactor layers static secret-prefix rules,
  Shannon-entropy detection, connection-string / credential key-value
  detection, and JSON-aware field skipping; opt-in PII detection and the
  behaviour mode are configured under `[agent.redaction]` in `.libra/config`
  (`mode = redact | warn | off`, default `redact`; `pii.email` / `pii.phone`).
  The full transcript blob is always force-redacted regardless of `mode`.
- Hook and capture diagnostics are best-effort and are designed to report
  actionable installation state rather than silently ignoring missing providers.
