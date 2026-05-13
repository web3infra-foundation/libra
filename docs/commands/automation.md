# `libra automation`

Manage AI automation rules for the current repository.

## Synopsis

```bash
libra automation list
libra automation run [--rule <id>] [--now <rfc3339>] [--live]
libra automation history [--limit <n>]
```

## Description

`libra automation` reads the repository automation configuration, evaluates
cron-style rules, and records execution history in the repository database.
By default, `run` uses a dry-run executor so shell actions are planned and
recorded without spawning external commands. Pass `--live` only when the
configured actions should actually run.

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `list` | Validate and list configured automation rules |
| `run` | Run due rules, or one named rule with `--rule <id>` |
| `history` | Show recent automation run history |

## Options

| Flag | Subcommand | Description |
|------|------------|-------------|
| `--rule <id>` | `run` | Run a single rule regardless of whether its trigger is due |
| `--now <rfc3339>` | `run` | Evaluate due rules against a simulated current time |
| `--live` | `run` | Execute shell actions that pass safety preflight |
| `--limit <n>` | `history` | Number of history rows to show; defaults to `20` |

## Human Output

`list` prints one tab-separated row per rule:

```text
<rule-id>	<trigger-kind>	<action-kind>
```

`run` prints one tab-separated row per result:

```text
<rule-id>	<status>	<message>
```

`history` includes the finished timestamp:

```text
<finished-at>	<rule-id>	<status>	<message>
```

## JSON Output

`--json` uses command-specific envelopes:

- `automation.list`
- `automation.run`
- `automation.history`

Example:

```json
{
  "ok": true,
  "command": "automation.run",
  "data": {
    "results": []
  }
}
```

## Notes

- The command requires a Libra repository for `run` and `history` because
  history is stored in `.libra/libra.db`.
- Configuration validation errors are surfaced before rules run.
- `--live` is intentionally explicit; dry-run remains the default.
