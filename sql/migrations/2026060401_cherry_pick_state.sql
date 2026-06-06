CREATE TABLE IF NOT EXISTS `cherry_pick_state` (
    `id`          INTEGER PRIMARY KEY AUTOINCREMENT,
    `head_name`   TEXT NOT NULL,
    `head_orig`   TEXT NOT NULL,
    `current_oid` TEXT NOT NULL,
    `todo`        TEXT NOT NULL,
    `opts_json`   TEXT NOT NULL,
    `updated_at`  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
