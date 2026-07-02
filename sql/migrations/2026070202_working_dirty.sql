-- Dirty-set cache (lore.md 1.1): advisory snapshot of the working tree's
-- dirty paths plus the staged set, rebuilt atomically by `status --scan` and
-- consumed ONLY by the opt-in `status --cached`/`--check-dirty`/`libra dirty`
-- surfaces. Default `status` never reads or writes these tables. Correctness
-- never depends on the cache: freshness is keyed on the index fingerprint +
-- HEAD OID recorded in `working_dirty_meta`, and any mismatch degrades to the
-- full reconcile. Owner API: `internal::dirty::DirtyCache` (single writer/reader).
CREATE TABLE IF NOT EXISTS `working_dirty` (
    `id`          INTEGER PRIMARY KEY AUTOINCREMENT,
    `path`        TEXT NOT NULL,             -- repo-relative, '/'-separated
    `kind`        TEXT NOT NULL DEFAULT 'unknown',
    -- unstaged: new|modified|deleted ; staged snapshot: staged_new|staged_modified|staged_deleted ;
    -- manual marks: unknown (classified in memory at read time)
    `source`      TEXT NOT NULL,             -- scan|manual|check
    `marked_at`   TEXT NOT NULL,             -- ISO-8601 UTC
    `verified_at` TEXT,
    UNIQUE(`path`, `kind`)
);
CREATE TABLE IF NOT EXISTS `working_dirty_meta` (
    `id`                INTEGER PRIMARY KEY CHECK (`id` = 1),
    `state`             TEXT NOT NULL DEFAULT 'stale',  -- fresh|stale
    `index_fingerprint` TEXT,   -- hex of the index trailing checksum (width = active hash kind); 'absent' when no index
    `head_oid`          TEXT,   -- HEAD commit at scan time (staged snapshot validity keys on BOTH)
    `scanned_at`        TEXT,
    `scan_lock_pid`     INTEGER,
    `scan_lock_at`      TEXT
);
