CREATE TABLE IF NOT EXISTS `notes` (
    `id`         INTEGER PRIMARY KEY AUTOINCREMENT,
    `notes_ref`  TEXT NOT NULL,
    `object`     TEXT NOT NULL,
    `blob`       TEXT NOT NULL,
    UNIQUE(`notes_ref`, `object`)
);

CREATE INDEX IF NOT EXISTS idx_notes_ref ON `notes`(`notes_ref`);
