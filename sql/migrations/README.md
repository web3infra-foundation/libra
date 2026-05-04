# Schema migrations directory

This directory holds **versioned, idempotent SQL migrations** managed by
`crate::internal::db::migration::MigrationRunner` (CEX-12.5).

## Filename convention

```
NNNN_<snake_case_name>.sql        # forward (up) migration
NNNN_<snake_case_name>.down.sql   # optional matching rollback
```

- `NNNN` is a zero-padded 4-digit monotonic version (`0001`, `0002`, ...).
- `<snake_case_name>` mirrors the migration's `name` field passed to
  `Migration { name: "...", .. }`.
- Forward migrations are required; `.down.sql` files are optional. A
  migration without a `.down.sql` cannot be rolled back through.

## Idempotency requirement

Forward DDL **must be idempotent** at the SQL level:

- `CREATE TABLE IF NOT EXISTS ...` (never bare `CREATE TABLE`)
- `CREATE INDEX IF NOT EXISTS ...`
- `ALTER TABLE ... ADD COLUMN` is OK only when guarded by a column-exists
  check (sqlite-specific) or scoped behind a feature flag.

Rationale: legacy databases initialized via `sqlite_20260309_init.sql` may
already contain tables that an early migration tries to create. Idempotent
DDL means the runner can safely apply every migration on every connect; the
`schema_versions` table is the bookkeeping layer, not the safety layer.

## Transaction-unsafe DDL is forbidden

The runner wraps every `up` and `down` DDL body in a SQLite transaction so
the schema change and the `schema_versions` insert/delete are atomic. SQLite
does not allow these statement types inside a transaction:

- `VACUUM` and `VACUUM INTO ...`
- Explicit `BEGIN` / `COMMIT` / `ROLLBACK` (the runner already manages this
  layer)
- `PRAGMA journal_mode = ...`, `PRAGMA wal_checkpoint`, and any other
  PRAGMA documented as transaction-sensitive

If a future CEX needs one of these, it must run the statement **outside**
the migration runner (e.g., in a dedicated maintenance command) and have
the migration only flip schema state.

## Don't reuse legacy `ensure_*_schema` table names without verification

The four legacy helpers in `src/internal/db.rs`
(`ensure_config_kv_schema`, `ensure_ai_projection_schema`,
`ensure_ai_runtime_contract_schema`, plus the bootstrap
`sqlite_20260309_init.sql`) own their tables. A new migration whose `up`
DDL targets one of those tables but ships a different shape will silently
no-op against legacy DBs (because of `IF NOT EXISTS`) and create a hidden
schema drift between fresh and legacy installs.

If a CEX must touch a legacy-owned table, it should:

1. First run a `PRAGMA table_info(<name>)` (or sea-orm equivalent) inside
   the migration to detect the shape; bail out with a clear error if it
   differs from what the migration assumes.
2. Or, preferred: leave the table alone and create a *new* table that
   joins back to the legacy one by id. Future CEX-15 / CEX-16 should
   default to this pattern.

## Registering migrations in code

The runner does **not** auto-load files from this directory at runtime
(SQLite migrations are compile-time critical and we want them embedded in
the binary). Instead, every migration is registered in
`crate::internal::db::migration::builtin_migrations` via
`include_str!("../../sql/migrations/<file>.sql")`.

When adding a new migration:

1. Drop the SQL into `sql/migrations/NNNN_<name>.sql` (and optionally
   `NNNN_<name>.down.sql`).
2. Add a corresponding entry to `builtin_migrations()` in
   `src/internal/db/migration.rs`, with the SQL embedded via
   `include_str!`. **Path**: from `src/internal/db/migration.rs` the
   correct relative path is `../../../sql/migrations/<file>.sql` (three
   `..` segments to escape `src/internal/db/`, then descend into
   `sql/migrations/`). Compare against the existing
   `src/internal/db.rs:include_str!("../../sql/sqlite_20260309_init.sql")`
   which sits one directory shallower and uses two `..` segments. The
   version number must be strictly greater than the previous one (the
   runner enforces this at registration time).
3. Add a unit / integration test under `tests/db_migration_test.rs`
   verifying the new table / column appears after `run_pending` and that a
   second `run_pending` is a no-op.

## CEX-12.5 initial state

CEX-12.5 ships the framework with **zero registered migrations**. The
`builtin_migrations()` registry is empty; the existing legacy schema
remains owned by `sqlite_20260309_init.sql` and the `ensure_*_schema`
helpers in `db.rs`. Future CEXes (13b, 15, 16, …) populate this directory
and the registry.
