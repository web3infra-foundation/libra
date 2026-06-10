mod archive_smoke;
mod branch_switch_checkout;
mod clean_rm_mv_lfs_basic;
mod clone_fetch_pull_local;
mod commit_status_log;
mod config_basic_kv;
mod config_get_default_and_patterns;
mod config_git_compat_mode;
mod config_import_path_edit;
mod config_key_generation;
mod config_list_variants;
mod config_scopes;
mod config_set_input_and_encryption;
mod config_unset_compat_flags;
mod cross_cutting_flags;
mod fetch_depth_local;
mod gc_smoke;
mod grep_blame_describe_shortlog;
mod init_bare_and_shared;
mod init_basic;
mod init_branch_and_format_options;
mod init_directory_and_quiet;
mod init_from_git_repository;
mod init_template;
mod init_vault;
mod live_github_create_push_clone_fetch;
mod merge_conflict_continue;
mod merge_rebase_cherry_revert_smoke;
mod object_readback;
mod open_smoke;
mod push_local_file_remote_rejected;
mod rebase_conflict_continue;
mod reflog_symbolic_ref;
mod restore_reset_diff;
mod schema_upgrade_observable;
mod sha256_object_readback;
mod shared;
mod stash_bisect_worktree;
mod tag_basic;
mod verify_pack_smoke;

pub(crate) mod prelude {
    pub(crate) use std::{fs, path::Path};

    pub(crate) use anyhow::{Context, Result, bail};

    pub(crate) use super::shared::create_committed_repo;
    pub(crate) use crate::{
        cleanup::GhRepoCleanupGuard,
        repo_root,
        runner::ScenarioCtx,
        support::{
            assert_json_error_code, assert_json_ok, assert_lbr_or_text, assert_not_contains,
            assert_stdout_contains, ensure_file, stdout_trim,
        },
    };
}

pub(crate) use archive_smoke::scenario_archive_smoke;
pub(crate) use branch_switch_checkout::scenario_branch_switch_checkout;
pub(crate) use clean_rm_mv_lfs_basic::scenario_clean_rm_mv_lfs_basic;
pub(crate) use clone_fetch_pull_local::scenario_clone_fetch_pull_local;
pub(crate) use commit_status_log::scenario_commit_status_log;
pub(crate) use config_basic_kv::scenario_config_basic_kv;
pub(crate) use config_get_default_and_patterns::scenario_config_get_default_and_patterns;
pub(crate) use config_git_compat_mode::scenario_config_git_compat_mode;
pub(crate) use config_import_path_edit::scenario_config_import_path_edit;
pub(crate) use config_key_generation::scenario_config_key_generation;
pub(crate) use config_list_variants::scenario_config_list_variants;
pub(crate) use config_scopes::scenario_config_scopes;
pub(crate) use config_set_input_and_encryption::scenario_config_set_input_and_encryption;
pub(crate) use config_unset_compat_flags::scenario_config_unset_compat_flags;
pub(crate) use cross_cutting_flags::scenario_cross_cutting_flags;
pub(crate) use fetch_depth_local::scenario_fetch_depth_local;
pub(crate) use gc_smoke::scenario_gc_smoke;
pub(crate) use grep_blame_describe_shortlog::scenario_grep_blame_describe_shortlog;
pub(crate) use init_bare_and_shared::scenario_init_bare_and_shared;
pub(crate) use init_basic::scenario_init_basic;
pub(crate) use init_branch_and_format_options::scenario_init_branch_and_format_options;
pub(crate) use init_directory_and_quiet::scenario_init_directory_and_quiet;
pub(crate) use init_from_git_repository::scenario_init_from_git_repository;
pub(crate) use init_template::scenario_init_template;
pub(crate) use init_vault::scenario_init_vault;
pub(crate) use live_github_create_push_clone_fetch::scenario_live_github_create_push_clone_fetch;
pub(crate) use merge_conflict_continue::scenario_merge_conflict_continue;
pub(crate) use merge_rebase_cherry_revert_smoke::scenario_merge_rebase_cherry_revert_smoke;
pub(crate) use object_readback::scenario_object_readback;
pub(crate) use open_smoke::scenario_open_smoke;
pub(crate) use push_local_file_remote_rejected::scenario_push_local_file_remote_rejected;
pub(crate) use rebase_conflict_continue::scenario_rebase_conflict_continue;
pub(crate) use reflog_symbolic_ref::scenario_reflog_symbolic_ref;
pub(crate) use restore_reset_diff::scenario_restore_reset_diff;
pub(crate) use schema_upgrade_observable::scenario_schema_upgrade_observable;
pub(crate) use sha256_object_readback::scenario_sha256_object_readback;
pub(crate) use stash_bisect_worktree::scenario_stash_bisect_worktree;
pub(crate) use tag_basic::scenario_tag_basic;
pub(crate) use verify_pack_smoke::scenario_verify_pack_smoke;
