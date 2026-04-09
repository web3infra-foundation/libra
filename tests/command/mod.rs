//! Shared test utilities and re-exports for the command integration test suite.

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
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
    utils::{
        pager::LIBRA_TEST_ENV,
        test::{self, ChangeDirGuard},
    },
};
use serde::Deserialize;
use serde_json::Value;
use serial_test::serial;
use tempfile::tempdir;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct CliErrorReport {
    pub(crate) error_code: String,
    pub(crate) category: String,
    pub(crate) exit_code: i32,
    pub(crate) severity: String,
    pub(crate) message: String,
    pub(crate) usage: Option<String>,
    #[serde(default)]
    pub(crate) hints: Vec<String>,
}

/// Run the Libra binary with an isolated HOME so host config never leaks into tests.
fn base_libra_command(args: &[&str], cwd: &Path) -> Command {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated config directory");

    let mut command = Command::new(env!("CARGO_BIN_EXE_libra"));
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env(LIBRA_TEST_ENV, "1");
    command
}

/// Run the Libra binary with an isolated HOME so host config never leaks into tests.
fn run_libra_command(args: &[&str], cwd: &Path) -> Output {
    base_libra_command(args, cwd)
        .output()
        .expect("failed to execute libra binary")
}

fn run_libra_command_with_stdin(args: &[&str], cwd: &Path, stdin_body: &str) -> Output {
    let mut child = base_libra_command(args, cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to execute libra binary");

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_body.as_bytes())
            .expect("failed to write stdin to libra process");
    }

    child
        .wait_with_output()
        .expect("failed to collect libra command output")
}

fn run_libra_command_with_stdin_and_env(
    args: &[&str],
    cwd: &Path,
    stdin_body: &str,
    extra_env: &[(&str, &str)],
) -> Output {
    let mut command = base_libra_command(args, cwd);
    for (key, value) in extra_env {
        command.env(key, value);
    }

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to execute libra binary");

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_body.as_bytes())
            .expect("failed to write stdin to libra process");
    }

    child
        .wait_with_output()
        .expect("failed to collect libra command output")
}

/// Assert that a CLI command succeeded and include stderr in the failure output.
fn assert_cli_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Split a structured CLI error into the human-readable block and the JSON report.
fn parse_cli_error_stderr(stderr: &[u8]) -> (String, CliErrorReport) {
    let stderr = String::from_utf8_lossy(stderr).to_string();
    let trimmed = stderr.trim_end();
    if let Ok(report) = serde_json::from_str::<CliErrorReport>(trimmed) {
        return (String::new(), report);
    }

    let json_start = trimmed
        .rfind("\n{")
        .map(|index| index + 1)
        .or_else(|| trimmed.find('{'))
        .expect("expected structured CLI stderr to contain a JSON report");
    let (human, json) = trimmed.split_at(json_start);
    let report: CliErrorReport =
        serde_json::from_str(json.trim()).expect("expected stderr JSON report to be valid JSON");
    (human.trim_end().to_string(), report)
}

fn parse_json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("expected stdout to be valid JSON")
}

/// Build the on-disk path to a loose object given the repository root and full
/// hex hash. Used by tests that need to corrupt or delete individual objects.
fn loose_object_path(repo: &Path, hash: &str) -> std::path::PathBuf {
    repo.join(libra::utils::util::ROOT_DIR)
        .join("objects")
        .join(&hash[..2])
        .join(&hash[2..])
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

#[cfg(unix)]
fn skip_permission_denied_test_if_root(test_name: &str) -> bool {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }

    // SAFETY: On Unix targets libc exposes `geteuid()` with no arguments and a
    // numeric return type compatible with `u32` on the platforms this suite runs on.
    let is_root = unsafe { geteuid() == 0 };
    if is_root {
        eprintln!(
            "skipping {test_name}: permission-based write failure injection is unreliable as root"
        );
    }

    is_root
}

mod add_cli_test;
mod add_json_test;
mod add_test;
mod bisect_test;
mod blame_test;
mod branch_test;
mod cat_file_test;
mod checkout_test;
mod cherry_pick_test;
mod clean_test;
mod cli_error_test;
mod clone_cli_test;
mod clone_test;
mod cloud_test;
#[cfg(unix)]
mod code_claudecode_test;
mod commit_error_test;
mod commit_json_test;
mod commit_test;
mod config_test;
mod describe_test;
mod diff_test;
mod fetch_test;
mod grep_test;
mod index_pack_test;
mod init_from_git_test;
mod init_json_test;
mod init_separate_libra_dir_test;
mod init_test;
mod lfs_test;
mod log_test;
mod merge_test;
mod mv_test;
mod open_test;
mod output_flags_test;
mod pull_json_test;
mod pull_test;
mod push_error_test;
mod push_json_test;
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
mod stash_test;
mod status_error_test;
mod status_json_test;
mod status_test;
mod switch_error_test;
mod switch_json_test;
mod switch_test;
mod tag_test;
#[cfg(all(unix, feature = "worktree-fuse"))]
mod worktree_fuse_test;
mod worktree_test;
