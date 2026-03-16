//! Shared test utilities and re-exports for the command integration test suite.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use git_internal::{
    hash::ObjectHash,
    internal::object::{commit::Commit, tree::Tree},
};
use libra::{
    command::{
        add::{self, AddArgs},
        branch::{BranchArgs, execute, filter_branches},
        calc_file_blob_hash,
        clean::{self, CleanArgs},
        commit::{self, CommitArgs, execute_safe},
        get_target_commit,
        init::{InitArgs, init},
        load_object,
        log::{LogArgs, get_reachable_commits},
        mv::{self, MvArgs},
        remove::{self, RemoveArgs},
        save_object,
        shortlog::{self, ShortlogArgs},
        status::{changes_to_be_committed, changes_to_be_staged},
        switch::{self, SwitchArgs},
    },
    common_utils::format_commit_msg,
    internal::{branch::Branch, head::Head},
    utils::test::{self, ChangeDirGuard},
};
use serial_test::serial;
use tempfile::tempdir;

/// Run the Libra binary with an isolated HOME so host config never leaks into tests.
fn run_libra_command(args: &[&str], cwd: &Path) -> Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated config directory");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .expect("failed to execute libra binary")
}

/// Assert that a CLI command succeeded and include stderr in the failure output.
fn assert_cli_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Initialize a repository through the CLI to exercise the real process entrypoint.
fn init_repo_via_cli(repo: &Path) {
    fs::create_dir_all(repo).expect("failed to create repository directory");
    let output = run_libra_command(&["init"], repo);
    assert_cli_success(&output, "failed to initialize repository");
}

/// Configure a stable local identity for commands that require commits.
fn configure_identity_via_cli(repo: &Path) {
    let output = run_libra_command(&["config", "user.name", "Test User"], repo);
    assert_cli_success(&output, "failed to configure user.name");

    let output = run_libra_command(&["config", "user.email", "test@example.com"], repo);
    assert_cli_success(&output, "failed to configure user.email");
}

/// Create a committed repository that is ready for branch, tag, and remote tests.
fn create_committed_repo_via_cli() -> tempfile::TempDir {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    fs::write(repo.path().join("tracked.txt"), "tracked\n").expect("failed to create tracked file");

    let output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&output, "failed to add tracked file");

    let output = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&output, "failed to create initial commit");

    repo
}

mod add_cli_test;
mod add_test;
mod blame_test;
mod branch_test;
mod cat_file_test;
mod checkout_test;
mod cherry_pick_test;
mod claude_sdk_test;
mod clean_test;
mod cli_error_test;
mod clone_cli_test;
mod clone_test;
mod cloud_test;
mod commit_test;
mod config_test;
mod diff_test;
mod fetch_test;
mod index_pack_test;
mod init_from_git_test;
mod init_separate_libra_dir_test;
mod init_test;
mod lfs_test;
mod log_test;
mod merge_test;
mod mv_test;
mod open_test;
mod pull_test;
mod push_test;
mod rebase_test;
mod reflog_test;
mod remote_test;
mod remove_test;
mod reset_test;
mod restore_test;
mod revert_test;
mod shortlog_test;
mod show_ref_test;
mod show_test;
mod status_test;
mod switch_test;
mod tag_test;
mod vault_cli_test;
mod vault_test;
mod worktree_test;
