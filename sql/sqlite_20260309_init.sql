-- SQLite bootstrap schema for Libra repositories.
-- This file supersedes the historical bootstrap filename
-- `sql/sqlite_20240331_init.sql`.

CREATE TABLE IF NOT EXISTS `config` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    `configuration` TEXT NOT NULL,
    `name` TEXT,
    `key` TEXT NOT NULL,
    `value` TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS `config_kv` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    `key` TEXT NOT NULL,
    `value` TEXT NOT NULL,
    `encrypted` INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_config_kv_key ON config_kv(`key`);
CREATE TABLE IF NOT EXISTS `reference` (
    `id` INTEGER PRIMARY KEY AUTOINCREMENT,
    -- name can't be ''
    `name` TEXT CHECK (name <> '' OR name IS NULL),
    `kind` TEXT NOT NULL CHECK (kind IN ('Branch', 'Tag', 'Head')),
    `commit` TEXT,
    -- remote can't be ''. If kind is Tag, remote must be NULL.
    `remote` TEXT CHECK (remote <> '' OR remote IS NULL),
    CHECK (
        (kind <> 'Tag' OR remote IS NULL)
    )
);
CREATE TABLE IF NOT EXISTS `reflog` (
    `id`              INTEGER PRIMARY KEY AUTOINCREMENT,
    `ref_name`        TEXT NOT NULL,
    `old_oid`         TEXT NOT NULL,
    `new_oid`         TEXT NOT NULL,
    `committer_name`  TEXT NOT NULL,
    `committer_email` TEXT NOT NULL,
    `timestamp`       INTEGER NOT NULL,
    `action`          TEXT NOT NULL,
    `message`         TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS `rebase_state` (
    `id`           INTEGER PRIMARY KEY AUTOINCREMENT,
    `head_name`    TEXT NOT NULL,
    `onto`         TEXT NOT NULL,
    `orig_head`    TEXT NOT NULL,
    `current_head` TEXT NOT NULL,
    `todo`         TEXT NOT NULL,
    `done`         TEXT NOT NULL,
    `stopped_sha`  TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_name_kind_remote ON `reference`(`name`, `kind`, `remote`)
WHERE `remote` IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_name_kind ON `reference`(`name`, `kind`)
WHERE `remote` IS NULL;

CREATE INDEX IF NOT EXISTS idx_ref_name_timestamp ON `reflog`(`ref_name`, `timestamp`);

CREATE TABLE IF NOT EXISTS `object_index` (
    `id`         INTEGER PRIMARY KEY AUTOINCREMENT,
    `o_id`       TEXT NOT NULL,
    `o_type`     TEXT NOT NULL,
    `o_size`     INTEGER NOT NULL,
    `repo_id`    TEXT NOT NULL,
    `created_at` INTEGER NOT NULL,
    `is_synced`  INTEGER DEFAULT 0,
    UNIQUE(`repo_id`, `o_id`)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_object_repo_oid ON `object_index`(`repo_id`, `o_id`);
CREATE INDEX IF NOT EXISTS idx_object_sync ON `object_index`(`repo_id`, `is_synced`);

-- BEGIN AI PROJECTION SCHEMA
CREATE TABLE IF NOT EXISTS `ai_thread` (
    `thread_id` TEXT PRIMARY KEY,
    `title` TEXT,
    `owner_kind` TEXT NOT NULL,
    `owner_id` TEXT NOT NULL,
    `owner_display_name` TEXT,
    `current_intent_id` TEXT,
    `latest_intent_id` TEXT,
    `metadata_json` TEXT,
    `archived` INTEGER NOT NULL DEFAULT 0 CHECK (`archived` IN (0, 1)),
    `version` INTEGER NOT NULL DEFAULT 0,
    `created_at` INTEGER NOT NULL,
    `updated_at` INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ai_thread_latest_intent ON `ai_thread`(`latest_intent_id`);
CREATE INDEX IF NOT EXISTS idx_ai_thread_current_intent ON `ai_thread`(`current_intent_id`);
CREATE INDEX IF NOT EXISTS idx_ai_thread_archived_updated ON `ai_thread`(`archived`, `updated_at`);

CREATE TABLE IF NOT EXISTS `ai_thread_participant` (
    `thread_id` TEXT NOT NULL,
    `actor_kind` TEXT NOT NULL,
    `actor_id` TEXT NOT NULL,
    `actor_display_name` TEXT,
    `role` TEXT NOT NULL,
    `joined_at` INTEGER NOT NULL,
    PRIMARY KEY (`thread_id`, `actor_kind`, `actor_id`),
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ai_thread_participant_actor
    ON `ai_thread_participant`(`actor_kind`, `actor_id`);

CREATE TABLE IF NOT EXISTS `ai_thread_intent` (
    `thread_id` TEXT NOT NULL,
    `intent_id` TEXT NOT NULL,
    `ordinal` INTEGER NOT NULL,
    `is_head` INTEGER NOT NULL DEFAULT 0 CHECK (`is_head` IN (0, 1)),
    `linked_at` INTEGER NOT NULL,
    `link_reason` TEXT NOT NULL,
    PRIMARY KEY (`thread_id`, `intent_id`),
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_thread_intent_thread_ordinal
    ON `ai_thread_intent`(`thread_id`, `ordinal`);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ai_thread_intent_intent
    ON `ai_thread_intent`(`intent_id`);
CREATE INDEX IF NOT EXISTS idx_ai_thread_intent_head
    ON `ai_thread_intent`(`thread_id`, `is_head`);

CREATE TABLE IF NOT EXISTS `ai_scheduler_state` (
    `thread_id` TEXT PRIMARY KEY,
    `selected_plan_id` TEXT,
    `active_task_id` TEXT,
    `active_run_id` TEXT,
    `metadata_json` TEXT,
    `version` INTEGER NOT NULL DEFAULT 0,
    `updated_at` INTEGER NOT NULL,
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ai_scheduler_selected_plan
    ON `ai_scheduler_state`(`selected_plan_id`);
CREATE INDEX IF NOT EXISTS idx_ai_scheduler_active_task
    ON `ai_scheduler_state`(`active_task_id`);
CREATE INDEX IF NOT EXISTS idx_ai_scheduler_active_run
    ON `ai_scheduler_state`(`active_run_id`);

CREATE TABLE IF NOT EXISTS `ai_scheduler_plan_head` (
    `thread_id` TEXT NOT NULL,
    `plan_id` TEXT NOT NULL,
    `ordinal` INTEGER NOT NULL,
    PRIMARY KEY (`thread_id`, `plan_id`),
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_scheduler_plan_head_thread_ordinal
    ON `ai_scheduler_plan_head`(`thread_id`, `ordinal`);

CREATE TABLE IF NOT EXISTS `ai_scheduler_selected_plan` (
    `thread_id` TEXT NOT NULL,
    `plan_id` TEXT NOT NULL,
    `ordinal` INTEGER NOT NULL,
    PRIMARY KEY (`thread_id`, `plan_id`),
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_scheduler_selected_plan_thread_ordinal
    ON `ai_scheduler_selected_plan`(`thread_id`, `ordinal`);
CREATE INDEX IF NOT EXISTS idx_ai_scheduler_selected_plan_plan
    ON `ai_scheduler_selected_plan`(`plan_id`);

CREATE TABLE IF NOT EXISTS `ai_live_context_window` (
    `thread_id` TEXT NOT NULL,
    `context_frame_id` TEXT NOT NULL,
    `position` INTEGER NOT NULL,
    `source_kind` TEXT NOT NULL,
    `pin_kind` TEXT,
    `inserted_at` INTEGER NOT NULL,
    PRIMARY KEY (`thread_id`, `context_frame_id`),
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_live_context_window_thread_position
    ON `ai_live_context_window`(`thread_id`, `position`);
CREATE INDEX IF NOT EXISTS idx_ai_live_context_window_frame
    ON `ai_live_context_window`(`context_frame_id`);

CREATE TABLE IF NOT EXISTS `ai_index_intent_plan` (
    `intent_id` TEXT NOT NULL,
    `plan_id` TEXT NOT NULL,
    `created_at` INTEGER NOT NULL,
    PRIMARY KEY (`intent_id`, `plan_id`)
);

CREATE TABLE IF NOT EXISTS `ai_index_intent_task` (
    `intent_id` TEXT NOT NULL,
    `task_id` TEXT NOT NULL,
    `parent_task_id` TEXT,
    `origin_step_id` TEXT,
    `created_at` INTEGER NOT NULL,
    PRIMARY KEY (`intent_id`, `task_id`)
);
CREATE INDEX IF NOT EXISTS idx_ai_index_intent_task_parent
    ON `ai_index_intent_task`(`parent_task_id`);

CREATE TABLE IF NOT EXISTS `ai_index_plan_step_task` (
    `plan_id` TEXT NOT NULL,
    `task_id` TEXT NOT NULL,
    `step_id` TEXT NOT NULL,
    `created_at` INTEGER NOT NULL,
    PRIMARY KEY (`plan_id`, `task_id`)
);
CREATE INDEX IF NOT EXISTS idx_ai_index_plan_step_task_step
    ON `ai_index_plan_step_task`(`plan_id`, `step_id`);

CREATE TABLE IF NOT EXISTS `ai_index_task_run` (
    `task_id` TEXT NOT NULL,
    `run_id` TEXT NOT NULL,
    `is_latest` INTEGER NOT NULL DEFAULT 0 CHECK (`is_latest` IN (0, 1)),
    `created_at` INTEGER NOT NULL,
    PRIMARY KEY (`task_id`, `run_id`)
);
CREATE INDEX IF NOT EXISTS idx_ai_index_task_run_task_created
    ON `ai_index_task_run`(`task_id`, `created_at`);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_index_task_run_latest
    ON `ai_index_task_run`(`task_id`) WHERE `is_latest` = 1;

CREATE TABLE IF NOT EXISTS `ai_index_run_event` (
    `run_id` TEXT NOT NULL,
    `event_id` TEXT NOT NULL,
    `event_kind` TEXT NOT NULL,
    `is_latest` INTEGER NOT NULL DEFAULT 0 CHECK (`is_latest` IN (0, 1)),
    `created_at` INTEGER NOT NULL,
    PRIMARY KEY (`run_id`, `event_id`)
);
CREATE INDEX IF NOT EXISTS idx_ai_index_run_event_run_created
    ON `ai_index_run_event`(`run_id`, `created_at`);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_index_run_event_latest
    ON `ai_index_run_event`(`run_id`) WHERE `is_latest` = 1;

CREATE TABLE IF NOT EXISTS `ai_index_run_patchset` (
    `run_id` TEXT NOT NULL,
    `patchset_id` TEXT NOT NULL,
    `sequence` INTEGER NOT NULL,
    `is_latest` INTEGER NOT NULL DEFAULT 0 CHECK (`is_latest` IN (0, 1)),
    `created_at` INTEGER NOT NULL,
    PRIMARY KEY (`run_id`, `patchset_id`)
);
CREATE INDEX IF NOT EXISTS idx_ai_index_run_patchset_run_sequence
    ON `ai_index_run_patchset`(`run_id`, `sequence`);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_index_run_patchset_latest
    ON `ai_index_run_patchset`(`run_id`) WHERE `is_latest` = 1;

CREATE TABLE IF NOT EXISTS `ai_index_intent_context_frame` (
    `intent_id` TEXT NOT NULL,
    `context_frame_id` TEXT NOT NULL,
    `relation_kind` TEXT NOT NULL,
    `created_at` INTEGER NOT NULL,
    PRIMARY KEY (`intent_id`, `context_frame_id`, `relation_kind`)
);
CREATE INDEX IF NOT EXISTS idx_ai_index_intent_context_frame_relation
    ON `ai_index_intent_context_frame`(`intent_id`, `relation_kind`);

CREATE TABLE IF NOT EXISTS `ai_validation_report` (
    `report_id` TEXT PRIMARY KEY,
    `thread_id` TEXT NOT NULL,
    `run_id` TEXT,
    `policy_version` TEXT NOT NULL,
    `stale` INTEGER NOT NULL DEFAULT 0 CHECK (`stale` IN (0, 1)),
    `is_latest` INTEGER NOT NULL DEFAULT 0 CHECK (`is_latest` IN (0, 1)),
    `summary_json` TEXT NOT NULL,
    `created_at` INTEGER NOT NULL,
    `updated_at` INTEGER NOT NULL,
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ai_validation_report_thread_created
    ON `ai_validation_report`(`thread_id`, `created_at`);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_validation_report_latest
    ON `ai_validation_report`(`thread_id`) WHERE `is_latest` = 1;

CREATE TABLE IF NOT EXISTS `ai_risk_score_breakdown` (
    `breakdown_id` TEXT PRIMARY KEY,
    `thread_id` TEXT NOT NULL,
    `validation_report_id` TEXT,
    `policy_version` TEXT NOT NULL,
    `stale` INTEGER NOT NULL DEFAULT 0 CHECK (`stale` IN (0, 1)),
    `is_latest` INTEGER NOT NULL DEFAULT 0 CHECK (`is_latest` IN (0, 1)),
    `summary_json` TEXT NOT NULL,
    `created_at` INTEGER NOT NULL,
    `updated_at` INTEGER NOT NULL,
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ai_risk_score_breakdown_thread_created
    ON `ai_risk_score_breakdown`(`thread_id`, `created_at`);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_risk_score_breakdown_latest
    ON `ai_risk_score_breakdown`(`thread_id`) WHERE `is_latest` = 1;

CREATE TABLE IF NOT EXISTS `ai_decision_proposal` (
    `proposal_id` TEXT PRIMARY KEY,
    `thread_id` TEXT NOT NULL,
    `validation_report_id` TEXT,
    `risk_score_breakdown_id` TEXT,
    `policy_version` TEXT NOT NULL,
    `stale` INTEGER NOT NULL DEFAULT 0 CHECK (`stale` IN (0, 1)),
    `is_latest` INTEGER NOT NULL DEFAULT 0 CHECK (`is_latest` IN (0, 1)),
    `summary_json` TEXT NOT NULL,
    `created_at` INTEGER NOT NULL,
    `updated_at` INTEGER NOT NULL,
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ai_decision_proposal_thread_created
    ON `ai_decision_proposal`(`thread_id`, `created_at`);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_decision_proposal_latest
    ON `ai_decision_proposal`(`thread_id`) WHERE `is_latest` = 1;

CREATE TABLE IF NOT EXISTS `ai_thread_provider_metadata` (
    `thread_id` TEXT PRIMARY KEY,
    `legacy_session_id` TEXT,
    `provider_thread_id` TEXT,
    `provider_kind` TEXT,
    `metadata_json` TEXT,
    `updated_at` INTEGER NOT NULL,
    FOREIGN KEY (`thread_id`) REFERENCES `ai_thread`(`thread_id`) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ai_thread_provider_metadata_legacy_session
    ON `ai_thread_provider_metadata`(`legacy_session_id`);
CREATE INDEX IF NOT EXISTS idx_ai_thread_provider_metadata_provider_thread
    ON `ai_thread_provider_metadata`(`provider_thread_id`);
-- END AI PROJECTION SCHEMA
