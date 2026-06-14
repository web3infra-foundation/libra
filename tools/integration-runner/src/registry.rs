use anyhow::Result;

use crate::{runner::ScenarioCtx, scenarios::*};

pub(crate) type ScenarioFn = fn(&mut ScenarioCtx<'_>) -> Result<()>;

/// Single source of truth for which scenarios have Rust implementations in this runner.
///
/// To add a new scenario (see integration-test-plan.md §2.4 and AGENTS.md):
///
/// 1. Register in docs/development/integration-scenarios.yaml and docs/development/integration-scenarios/<id>.md.
/// 2. Implement `fn scenario_xxx(ctx: &mut ScenarioCtx) -> Result<()> { ... }` in src/scenarios/<id>.rs.
/// 3. Add `("cli.xxx", scenario_xxx),` to `scenario_registry()` below.
///
/// `check-plan` and the run dispatcher both derive *only* from this registry. There are no other match arms or const lists to keep in sync.
/// Keep this one-file-per-scenario layout: when a command's behavior changes, edit the owner scenario file instead of adding command-specific branches here.
pub(crate) fn scenario_registry() -> &'static [(&'static str, ScenarioFn)] {
    static REG: &[(&str, ScenarioFn)] = &[
        ("cli.init-basic", scenario_init_basic),
        ("cli.config-basic-kv", scenario_config_basic_kv),
        ("cli.config-scopes", scenario_config_scopes),
        (
            "cli.config-set-input-and-encryption",
            scenario_config_set_input_and_encryption,
        ),
        (
            "cli.config-get-default-and-patterns",
            scenario_config_get_default_and_patterns,
        ),
        ("cli.config-list-variants", scenario_config_list_variants),
        (
            "cli.config-unset-compat-flags",
            scenario_config_unset_compat_flags,
        ),
        (
            "cli.config-import-path-edit",
            scenario_config_import_path_edit,
        ),
        ("cli.config-key-generation", scenario_config_key_generation),
        (
            "cli.config-git-compat-mode",
            scenario_config_git_compat_mode,
        ),
        (
            "cli.init-directory-and-quiet",
            scenario_init_directory_and_quiet,
        ),
        (
            "cli.init-branch-and-format-options",
            scenario_init_branch_and_format_options,
        ),
        ("cli.init-bare-and-shared", scenario_init_bare_and_shared),
        ("cli.init-template", scenario_init_template),
        ("cli.init-vault", scenario_init_vault),
        (
            "cli.init-from-git-repository",
            scenario_init_from_git_repository,
        ),
        ("cli.commit-status-log", scenario_commit_status_log),
        (
            "cli.branch-switch-checkout",
            scenario_branch_switch_checkout,
        ),
        ("cli.restore-reset-diff", scenario_restore_reset_diff),
        ("cli.stash-bisect-worktree", scenario_stash_bisect_worktree),
        ("cli.tag-basic", scenario_tag_basic),
        (
            "cli.merge-rebase-cherry-revert-smoke",
            scenario_merge_rebase_cherry_revert_smoke,
        ),
        (
            "cli.merge-conflict-continue",
            scenario_merge_conflict_continue,
        ),
        (
            "cli.rebase-conflict-continue",
            scenario_rebase_conflict_continue,
        ),
        (
            "cli.grep-blame-describe-shortlog",
            scenario_grep_blame_describe_shortlog,
        ),
        ("cli.clean-rm-mv-lfs-basic", scenario_clean_rm_mv_lfs_basic),
        ("cli.reflog-symbolic-ref", scenario_reflog_symbolic_ref),
        ("cli.open-smoke", scenario_open_smoke),
        ("cli.cross-cutting-flags", scenario_cross_cutting_flags),
        ("cli.notes-smoke", scenario_notes_smoke),
        ("cli.object-readback", scenario_object_readback),
        ("cli.ls-tree-smoke", scenario_ls_tree_smoke),
        (
            "cli.sha256-object-readback",
            scenario_sha256_object_readback,
        ),
        (
            "cli.clone-fetch-pull-local",
            scenario_clone_fetch_pull_local,
        ),
        (
            "cli.push-local-file-remote-rejected",
            scenario_push_local_file_remote_rejected,
        ),
        ("cli.fetch-depth-local", scenario_fetch_depth_local),
        (
            "cli.schema-upgrade-observable",
            scenario_schema_upgrade_observable,
        ),
        ("cli.gc-smoke", scenario_gc_smoke),
        ("cli.archive-smoke", scenario_archive_smoke),
        ("cli.notes-smoke", scenario_notes_smoke),
        ("cli.verify-pack-smoke", scenario_verify_pack_smoke),
        // Wave 3 live (dispatched only via `run-live`; normal `run --waves 3` skips gh_required early)
        (
            "live.github-create-push-clone-fetch",
            scenario_live_github_create_push_clone_fetch,
        ),
    ];
    REG
}
