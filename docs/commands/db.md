# `libra db`

Inspect and upgrade the repository SQLite schema.

## Synopsis

```bash
libra db status
libra db upgrade
```

## Description

Libra stores repository metadata in `.libra/libra.db`. New Libra releases can add
tables, columns, or indexes needed by newer features. Normal repository commands
check the recorded schema version before opening the database. If the repository
schema is older than the running Libra binary, the command stops with
`LBR-REPO-002` and asks you to run `libra db upgrade`.

Database upgrades are explicit. Connecting to a repository database does not apply
pending migrations.

## Subcommands

| Subcommand | Description |
|------------|-------------|
| `status` | Print the current schema version and the latest version supported by this Libra binary without modifying the database. |
| `upgrade` | Apply pending built-in migrations for the current Libra binary. |

## Output

Human `upgrade` output reports whether any migrations were applied:

```text
Upgraded repository database schema from 2026050801 to 2026052301 (applied: 2026052301).
```

If no migrations are pending:

```text
Repository database schema is up to date (version 2026052301).
```

With `--json`, `db upgrade` emits:

```json
{
  "ok": true,
  "command": "db.upgrade",
  "data": {
    "previous_version": 2026050801,
    "current_version": 2026052301,
    "latest_version": 2026052301,
    "applied_versions": [2026052301],
    "upgraded": true
  }
}
```

## Examples

```bash
# Show the repository schema version (no writes)
libra db status

# Structured JSON output with current/latest version + state
libra db --json status

# Apply pending migrations to bring the schema to this Libra version
libra db upgrade

# Structured JSON output with applied_versions[] for the upgrade
libra db --json upgrade
```

The same banner is rendered by `libra db --help` so the doc and the
CLI surface stay in sync (cross-cutting `--help` EXAMPLES rollout, see
`docs/development/commands/_general.md` item B).

## Safety

- `db status` is read-only.
- `db upgrade` runs each migration inside the migration runner's transaction
  boundary and records the applied version in `schema_versions`.
- If the repository database was created by a newer Libra binary, older binaries
  refuse to run and ask you to install a newer Libra version instead of attempting
  a downgrade.
