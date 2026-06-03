//! Tests `libra branch` for creation, listing, deletion, renaming,
//! upstream tracking, and `--contains`/`--no-contains` filtering.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! Fixture conventions:
//! - CLI cases use `create_committed_repo_via_cli()` and exercise the
//!   binary so we cover error-code/exit-code surfaces (`LBR-CLI-003`,
//!   `LBR-REPO-002`, `LBR-REPO-003`, `LBR-IO-002`).
//! - In-process cases call `setup_with_new_libra_in()` plus an empty
//!   commit chain (`commit::execute` with `allow_empty=true`,
//!   `disable_pre=true`) and assert against `Branch::find_branch` /
//!   `Head::current()`.
//! - The `--contains` test builds a divergent two-branch graph (master:
//!   base/m1/m2, dev: base/d1/d2) and exhaustively exercises filter
//!   semantics. Several Unix-only cases force `permission-denied` writes
//!   on the SQLite file, so they `skip_permission_denied_test_if_root`.

#![cfg(test)]

use std::collections::HashSet;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use git_internal::hash::{ObjectHash, get_hash_kind};
use libra::internal::config::ConfigKv;
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Scenario: `libra branch <new> <bad-ref>` must reject the invalid start
/// point with exit 129 and a structured `LBR-CLI-003` error. Pins the CLI
/// usage error envelope.
#[test]
fn test_branch_cli_invalid_start_point_returns_cli_exit_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["branch", "new", "badref"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("fatal: not a valid object name: 'badref'"));
    assert!(stderr.contains("Error-Code: LBR-CLI-003"));
}

/// Scenario: `--json branch <name>` must emit `command="branch"`,
/// `data.action="create"`, `data.name=<name>` and a non-empty
/// `data.commit`. Schema pin for branch-create JSON output.
#[test]
fn test_branch_json_create_output_reports_branch() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "branch", "feature"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "branch");
    assert_eq!(json["data"]["action"], "create");
    assert_eq!(json["data"]["name"], "feature");
    assert!(json["data"]["commit"].as_str().is_some());
}

/// Scenario: human-readable branch creation must print "Created branch
/// 'feature' at <hash>" on stdout. Pins the confirmation message format.
#[test]
fn test_branch_create_outputs_confirmation() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&output, "branch feature");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Created branch 'feature' at "),
        "unexpected stdout: {stdout}"
    );
}

/// Scenario: in a freshly initialised repo with no commits but registered
/// remote refs, `branch -a` must still display the unborn HEAD (`* main`)
/// plus the remote ref. Regression guard against treating "unborn" as
/// "no branches".
#[tokio::test]
#[serial]
async fn test_branch_all_shows_unborn_head_even_with_remote_refs() {
    let repo = tempdir().unwrap();
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    let remote_add = run_libra_command(
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
        repo.path(),
    );
    assert_cli_success(&remote_add, "remote add origin");

    Branch::update_branch(
        "refs/remotes/origin/main",
        &ObjectHash::zero_str(get_hash_kind()),
        Some("origin"),
    )
    .await
    .unwrap();

    let output = run_libra_command(&["branch", "-a"], repo.path());
    assert_cli_success(&output, "branch -a on unborn repo with remotes");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("* main"),
        "expected unborn HEAD marker in stdout: {stdout}"
    );
    assert!(
        stdout.contains("origin/main"),
        "expected remote branch in stdout: {stdout}"
    );
}

/// Scenario: when `branch -d` targets a misspelled branch, the structured
/// error must include a "did you mean" suggestion based on existing branch
/// names. Pins the typo-suggestion contract (`LBR-CLI-003`, exit 129).
#[test]
fn test_branch_not_found_suggests_similar_name() {
    let repo = create_committed_repo_via_cli();

    let create = run_libra_command(&["branch", "featur"], repo.path());
    assert_cli_success(&create, "branch featur");

    let output = run_libra_command(&["branch", "-d", "feature"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        stderr.contains("did you mean 'featur'?"),
        "expected suggestion in stderr, got: {stderr}"
    );
}

/// Scenario: `branch --set-upstream-to` from detached HEAD must fail with
/// `LBR-REPO-003` (exit 128) and the message must mention "HEAD is
/// detached" plus a "checkout a branch first" hint. Pins both the error
/// tag and the user-facing remediation.
#[test]
fn test_branch_set_upstream_detached_head_returns_repo_state_error() {
    let repo = create_committed_repo_via_cli();

    let detach = run_libra_command(&["switch", "--detach", "HEAD"], repo.path());
    assert!(
        detach.status.success(),
        "detach failed: {}",
        String::from_utf8_lossy(&detach.stderr)
    );

    let output = run_libra_command(&["branch", "--set-upstream-to", "origin/main"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(stderr.contains("HEAD is detached"));
    assert!(stderr.contains("checkout a branch first"));
}

/// Scenario: `branch --set-upstream-to <remote>/<branch>` must reject a
/// remote name that has no configured `remote.<name>.url`. Previously this
/// silently wrote `branch.main.remote=origin`, leaving JSON consumers with a
/// successful set-upstream output that could not be used by later push/pull
/// flows.
#[test]
fn test_branch_set_upstream_rejects_unknown_remote() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["branch", "--set-upstream-to", "origin/main"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        stderr.contains("remote 'origin' not found"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("libra remote -v"),
        "missing remediation hint in stderr: {stderr}"
    );
}

/// Scenario (Unix only): if SQLite write permission is revoked
/// (`chmod 0o444`), `branch --set-upstream-to` must surface an
/// `LBR-IO-002` error mentioning the failing config key. The original
/// permission mode is restored before assertions to avoid TempDir
/// teardown failures. Skipped under root because the chmod injection
/// has no effect.
#[cfg(unix)]
#[test]
fn test_branch_set_upstream_surfaces_config_write_failure() {
    if skip_permission_denied_test_if_root("test_branch_set_upstream_surfaces_config_write_failure")
    {
        return;
    }

    let repo = create_committed_repo_via_cli();
    let remote_add = run_libra_command(
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
        repo.path(),
    );
    assert_cli_success(&remote_add, "remote add origin");
    let db_path = repo.path().join(".libra").join("libra.db");
    let original_mode = fs::metadata(&db_path).unwrap().permissions().mode();

    fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let output = run_libra_command(&["branch", "--set-upstream-to", "origin/main"], repo.path());
    fs::set_permissions(&db_path, std::fs::Permissions::from_mode(original_mode)).unwrap();

    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-IO-002");
    assert!(
        stderr.contains("failed to persist branch config 'branch.main.remote'"),
        "unexpected stderr: {stderr}"
    );
}

/// Scenario (Unix only): if the upstream is already configured, a
/// repeat `--set-upstream-to` call must NOT touch the config file. This
/// is verified by making the SQLite file read-only between invocations
/// and confirming the second call still succeeds. Pins the "no redundant
/// write" optimisation. Skipped under root.
#[cfg(unix)]
#[test]
fn test_branch_set_upstream_idempotent_path_skips_redundant_write() {
    if skip_permission_denied_test_if_root(
        "test_branch_set_upstream_idempotent_path_skips_redundant_write",
    ) {
        return;
    }

    let repo = create_committed_repo_via_cli();
    let remote_add = run_libra_command(
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
        repo.path(),
    );
    assert_cli_success(&remote_add, "remote add origin");

    let first = run_libra_command(&["branch", "--set-upstream-to", "origin/main"], repo.path());
    assert_cli_success(&first, "initial set-upstream");

    let db_path = repo.path().join(".libra").join("libra.db");
    let original_mode = fs::metadata(&db_path).unwrap().permissions().mode();

    fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let second = run_libra_command(&["branch", "--set-upstream-to", "origin/main"], repo.path());
    fs::set_permissions(&db_path, std::fs::Permissions::from_mode(original_mode)).unwrap();

    assert_cli_success(&second, "idempotent set-upstream");
}

/// Scenario: `branch -D <name>` must print "Deleted branch <name> (was
/// <hash>)" on stdout. Pins the force-delete confirmation message.
#[test]
fn test_branch_force_delete_outputs_confirmation() {
    let repo = create_committed_repo_via_cli();

    let create = run_libra_command(&["branch", "topic"], repo.path());
    assert_cli_success(&create, "branch topic");

    let output = run_libra_command(&["branch", "-D", "topic"], repo.path());
    assert_cli_success(&output, "branch -D topic");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Deleted branch topic (was "),
        "unexpected stdout: {stdout}"
    );
}

/// Scenario: `libra branch --help` must render the `--set-upstream-to` doc
/// in a readable form. Previously the doc comment carried garbled
/// backtick/angle-bracket escaping (`\`branchname\`>\`'s tracking …`)
/// which leaked verbatim into the help text. This guard pins the cleaned
/// wording and prevents the bad pattern from re-appearing.
#[test]
fn test_branch_set_upstream_help_is_readable() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["branch", "--help"], repo.path());
    assert_cli_success(&output, "branch --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("`>`'s"),
        "branch --help still contains garbled doc fragment `>`'s — see \
         src/command/branch.rs::set_upstream_to docstring. Got:\n{stdout}"
    );
    assert!(
        stdout.contains("tracking information"),
        "branch --help missing readable --set-upstream-to description. Got:\n{stdout}"
    );
}

/// Scenario: in-process happy path for branch creation:
/// 1. Two empty commits on `main` produce two distinct commit hashes.
/// 2. Creating `first_branch` at the older hash must record that hash.
/// 3. Creating `second_branch` without an explicit start point must
///    inherit the current HEAD hash.
/// Also exercises `--show-current` (output not asserted, just non-panic).
#[tokio::test]
#[serial]
async fn test_branch() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let commit_args = CommitArgs {
        message: Some("first".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;
    let first_commit_id = Branch::find_branch_result("main", None)
        .await
        .expect("failed to query main branch")
        .expect("main branch should exist")
        .commit;

    let commit_args = CommitArgs {
        message: Some("second".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;
    let second_commit_id = Branch::find_branch_result("main", None)
        .await
        .expect("failed to query main branch")
        .expect("main branch should exist")
        .commit;

    {
        // create branch with first commit
        let first_branch_name = "first_branch".to_string();
        let args = BranchArgs {
            new_branch: Some(first_branch_name.clone()),
            commit_hash: Some(first_commit_id.to_string()),
            list: false,
            delete: None,
            delete_safe: None,
            set_upstream_to: None,
            unset_upstream: None,
            show_current: false,
            rename: vec![],
            remotes: false,
            all: false,
            contains: vec![],
            no_contains: vec![],
            merged: None,
            no_merged: None,
            points_at: None,
            ignore_case: false,
            sort: None,
            format: None,
            copy: vec![],
            force_copy: vec![],
            edit_description: None,
            force: false,
            create_reflog: false,
            track: None,
            no_track: false,
        };
        execute(args).await;

        // check branch exist
        match Head::current().await {
            Head::Branch(current_branch) => {
                assert_ne!(current_branch, first_branch_name)
            }
            _ => panic!("should be branch"),
        };

        let first_branch = Branch::find_branch_result(&first_branch_name, None)
            .await
            .expect("failed to query first branch")
            .expect("first_branch should exist");
        assert_eq!(first_branch.commit, first_commit_id);
        assert_eq!(first_branch.name, first_branch_name);
    }

    {
        // create second branch with current branch
        let second_branch_name = "second_branch".to_string();
        let args = BranchArgs {
            new_branch: Some(second_branch_name.clone()),
            commit_hash: None,
            list: false,
            delete: None,
            delete_safe: None,
            set_upstream_to: None,
            unset_upstream: None,
            show_current: false,
            rename: vec![],
            remotes: false,
            all: false,
            contains: vec![],
            no_contains: vec![],
            merged: None,
            no_merged: None,
            points_at: None,
            ignore_case: false,
            sort: None,
            format: None,
            copy: vec![],
            force_copy: vec![],
            edit_description: None,
            force: false,
            create_reflog: false,
            track: None,
            no_track: false,
        };
        execute(args).await;
        let second_branch = Branch::find_branch_result(&second_branch_name, None)
            .await
            .expect("failed to query second branch")
            .expect("second_branch should exist");
        assert_eq!(second_branch.commit, second_commit_id);
        assert_eq!(second_branch.name, second_branch_name);
    }

    // show current branch
    println!("show current branch");
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: true,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    // list branches
    println!("list branches");
    // execute(BranchArgs::parse_from([""])).await; // default list
}

/// Scenario: a local branch can be created from `origin/main` (a
/// remote-tracking ref). Verifies the resulting branch points to the
/// same hash that the remote ref recorded.
#[tokio::test]
#[serial]
async fn test_create_branch_from_remote() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    let args = CommitArgs {
        message: Some("first".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(args).await;
    let hash = Head::current_commit().await.unwrap();
    Branch::update_branch("main", &hash.to_string(), Some("origin"))
        .await
        .unwrap(); // create remote branch
    assert!(get_target_commit("origin/main").await.is_ok());

    let args = BranchArgs {
        new_branch: Some("test_new".to_string()),
        commit_hash: Some("origin/main".into()),
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    let branch = Branch::find_branch_result("test_new", None)
        .await
        .expect("failed to query test_new branch")
        .expect("branch create failed found");
    assert_eq!(branch.commit, hash);
}

/// Scenario: branch creation accepts the fully-qualified
/// `refs/remotes/origin/main` form (in addition to the short `origin/main`
/// form covered by the previous test). Confirms ref resolution accepts
/// both spellings.
#[tokio::test]
#[serial]
async fn test_create_branch_from_remote_tracking_ref() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    commit::execute(CommitArgs {
        message: Some("first".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let hash = Head::current_commit().await.unwrap();
    Branch::update_branch(
        "refs/remotes/origin/main",
        &hash.to_string(),
        Some("origin"),
    )
    .await
    .unwrap();

    assert!(get_target_commit("origin/main").await.is_ok());

    execute(BranchArgs {
        new_branch: Some("tracking-copy".to_string()),
        commit_hash: Some("origin/main".into()),
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    })
    .await;

    let branch = Branch::find_branch_result("tracking-copy", None)
        .await
        .expect("failed to query tracking-copy branch")
        .expect("branch create from tracking ref failed");
    assert_eq!(branch.commit, hash);
}

/// Scenario: corrupt HEAD storage (the `main` ref points at a
/// non-existent hash) must surface as `LBR-REPO-002` (exit 128) when
/// trying to create a branch off HEAD, with messages "failed to resolve
/// HEAD commit" and "stored branch reference 'main' is corrupt". The
/// inner block uses a guard so the corruption is applied with the test
/// CWD set to the repo before reverting.
#[tokio::test]
#[serial]
async fn test_branch_create_without_base_surfaces_corrupt_head_storage() {
    let repo = create_committed_repo_via_cli();
    {
        let _guard = ChangeDirGuard::new(repo.path());
        Branch::update_branch("main", "not-a-valid-hash", None)
            .await
            .unwrap();
    }

    let output = run_libra_command(&["branch", "feature"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to resolve HEAD commit"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("stored branch reference 'main' is corrupt"),
        "unexpected stderr: {stderr}"
    );
}

/// Scenario: same corruption pattern as above, but exercised through
/// `branch -d`. The safe-delete path must also surface `LBR-REPO-002`
/// with the corrupt-HEAD message rather than crash or report a misleading
/// "branch not merged" error.
#[tokio::test]
#[serial]
async fn test_branch_delete_safe_surfaces_corrupt_head_storage() {
    let repo = create_committed_repo_via_cli();
    let create = run_libra_command(&["branch", "topic"], repo.path());
    assert_cli_success(&create, "branch topic");

    {
        let _guard = ChangeDirGuard::new(repo.path());
        Branch::update_branch("main", "not-a-valid-hash", None)
            .await
            .unwrap();
    }

    let output = run_libra_command(&["branch", "-d", "topic"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to resolve HEAD commit"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("stored branch reference 'main' is corrupt"),
        "unexpected stderr: {stderr}"
    );
}

/// Scenario: same corruption pattern, exercised through
/// `branch --show-current`. The display-only path must NOT silently
/// succeed when HEAD storage is broken; it must surface `LBR-REPO-002`.
#[tokio::test]
#[serial]
async fn test_branch_show_current_surfaces_corrupt_head_storage() {
    let repo = create_committed_repo_via_cli();
    {
        let _guard = ChangeDirGuard::new(repo.path());
        Branch::update_branch("main", "not-a-valid-hash", None)
            .await
            .unwrap();
    }

    let output = run_libra_command(&["branch", "--show-current"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to resolve HEAD commit"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("stored branch reference 'main' is corrupt"),
        "unexpected stderr: {stderr}"
    );
}

/// Scenario: a stray branch with an invalid commit hash
/// (`broken-topic`) must trip the listing path with `LBR-REPO-002` and a
/// "stored branch reference 'broken-topic' is corrupt" message. Confirms
/// listing validates every branch row, not only HEAD.
#[tokio::test]
#[serial]
async fn test_branch_list_surfaces_corrupt_reference_name() {
    let repo = create_committed_repo_via_cli();
    {
        let _guard = ChangeDirGuard::new(repo.path());
        Branch::update_branch("broken-topic", "not-a-valid-hash", None)
            .await
            .unwrap();
    }

    let output = run_libra_command(&["branch"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("stored branch reference 'broken-topic' is corrupt"),
        "unexpected stderr: {stderr}"
    );
}

/// Scenario: branch names rejected by `is_valid_git_branch_name`
/// (e.g. `@{mega}`) must not be created. Asserts both the validator's
/// return value and the post-condition that the branch does not exist.
#[tokio::test]
#[serial]
async fn test_invalid_branch_name() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    let args = CommitArgs {
        message: Some("first".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(args).await;

    // Check validation logic directly
    assert!(!libra::command::branch::is_valid_git_branch_name("@{mega}"));

    // Ensure no branch was created
    let branch = Branch::find_branch_result("@{mega}", None)
        .await
        .expect("failed to query @{mega} branch");
    assert!(branch.is_none(), "invalid branch should not be created");
}

/// Scenario: `branch -m old new` renames a non-current branch. Verifies
/// the old name no longer resolves and the new name carries the same
/// commit hash.
#[tokio::test]
#[serial]
async fn test_branch_rename() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    // Create initial commit
    let args = CommitArgs {
        message: Some("first".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(args).await;
    let commit_id_1 = Head::current_commit().await.unwrap();

    // Create a test branch
    let args = BranchArgs {
        new_branch: Some("old_name".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    // Verify old branch exists
    let old_branch = Branch::find_branch_result("old_name", None)
        .await
        .expect("failed to query old_name branch");
    assert!(old_branch.is_some(), "old branch should exist");
    assert_eq!(old_branch.unwrap().commit, commit_id_1);

    // Rename branch from old_name to new_name
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec!["old_name".to_string(), "new_name".to_string()],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    // Verify old branch no longer exists
    let old_branch = Branch::find_branch_result("old_name", None)
        .await
        .expect("failed to query old_name branch");
    assert!(
        old_branch.is_none(),
        "old branch should not exist after rename"
    );

    // Verify new branch exists with same commit
    let new_branch = Branch::find_branch_result("new_name", None)
        .await
        .expect("failed to query new_name branch");
    assert!(new_branch.is_some(), "new branch should exist");
    assert_eq!(new_branch.unwrap().commit, commit_id_1);
}

/// Scenario: renaming the currently checked-out branch must update HEAD
/// to the new name. Uses the single-argument `rename: vec![new]` form
/// which renames *the current* branch. Pins the HEAD-follows-rename
/// invariant.
#[tokio::test]
#[serial]
async fn test_rename_current_branch() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    // Create initial commit
    let args = CommitArgs {
        message: Some("first".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(args).await;
    let commit_id = Head::current_commit().await.unwrap();

    // Verify we're on main branch
    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "main"),
        _ => panic!("should be on a branch"),
    }

    // Create and switch to a feature branch
    let feature_branch = "feature".to_string();
    switch::execute(SwitchArgs {
        branch: None,
        create: Some(feature_branch.clone()),
        detach: false,
        track: false,
    })
    .await;

    // Verify we're on feature branch
    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "feature"),
        _ => panic!("should be on feature branch"),
    }

    // Rename current branch (feature) to feature_new using single argument
    let feature_new = "feature_new".to_string();
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![feature_new.clone()],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    // Verify HEAD is now on 'feature_new'
    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, feature_new),
        _ => panic!("should be on a branch"),
    }

    // Verify old branch no longer exists
    let old_branch = Branch::find_branch_result(&feature_branch, None)
        .await
        .expect("failed to query feature branch");
    assert!(
        old_branch.is_none(),
        "feature branch should not exist after rename"
    );

    // Verify new branch exists with same commit
    let new_branch = Branch::find_branch_result(&feature_new, None)
        .await
        .expect("failed to query feature_new branch");
    assert!(new_branch.is_some(), "feature_new branch should exist");
    assert_eq!(new_branch.unwrap().commit, commit_id);
}

/// Scenario: renaming `branch1` to `branch2` while `branch2` already
/// exists must fail and leave both branches intact. Pins the
/// "no overwrite without -M" guard.
#[tokio::test]
#[serial]
async fn test_rename_to_existing_branch() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    // Create initial commit
    let args = CommitArgs {
        message: Some("first".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(args).await;

    // Create two branches
    let args = BranchArgs {
        new_branch: Some("branch1".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    let args = BranchArgs {
        new_branch: Some("branch2".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    // Try to rename branch1 to branch2 (should fail)
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec!["branch1".to_string(), "branch2".to_string()],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    // Verify both branches still exist
    assert!(
        Branch::find_branch_result("branch1", None)
            .await
            .expect("failed to query branch1")
            .is_some()
    );
    assert!(
        Branch::find_branch_result("branch2", None)
            .await
            .expect("failed to query branch2")
            .is_some()
    );
}

/// Scenario: `branch -a` must list both local and remote branches
/// without crashing. The output is not directly captured (it just goes
/// to stdout); the assertion is that both local (`feature_branch`) and
/// remote (`origin/remote_branch`) refs resolve through `Branch::find_branch`.
#[tokio::test]
#[serial]
async fn test_list_all_branches() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    // Create initial commit
    let args = CommitArgs {
        message: Some("initial commit".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(args).await;

    // Create local branch
    let args = BranchArgs {
        new_branch: Some("feature_branch".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await;

    ConfigKv::set("remote.origin.url", "https://example.com/repo.git", false)
        .await
        .unwrap();

    // Create remote branch
    let hash = Head::current_commit().await.unwrap();
    Branch::update_branch("remote_branch", &hash.to_string(), Some("origin"))
        .await
        .unwrap();

    // Test -a parameter - just call execute, don't try to capture output
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: true,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    };
    execute(args).await; // This will print to stdout, which is fine for tests

    // Verify branches exist
    assert!(
        Branch::find_branch_result("main", None)
            .await
            .expect("failed to query main branch")
            .is_some()
    );
    assert!(
        Branch::find_branch_result("feature_branch", None)
            .await
            .expect("failed to query feature_branch")
            .is_some()
    );
    assert!(
        Branch::find_branch_result("remote_branch", Some("origin"))
            .await
            .expect("failed to query remote_branch")
            .is_some()
    );
}

/// Scenario: `branch -d <name>` must refuse to delete an unmerged branch
/// and succeed once the branch has been merged into the current head.
/// Uses a fast-forward "merge" by directly updating `main` to the
/// feature branch's commit. Pins the safe-delete merge gate.
#[tokio::test]
#[serial]
async fn test_branch_delete_safe() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create first commit on master
    let commit_args = CommitArgs {
        message: Some("initial commit".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;

    // Create a feature branch
    execute(BranchArgs {
        new_branch: Some("feature".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    })
    .await;

    // Switch to feature branch and make a commit
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    let commit_args = CommitArgs {
        message: Some("feature work".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;

    // Switch back to master
    switch::execute(SwitchArgs {
        branch: Some("main".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    // Try to delete feature branch with -d (should fail - not merged)
    execute(BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: Some("feature".to_string()),
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    })
    .await;

    // Feature branch should still exist
    assert!(
        Branch::find_branch_result("feature", None)
            .await
            .expect("failed to query feature branch")
            .is_some()
    );

    // Now merge feature into master
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    switch::execute(SwitchArgs {
        branch: Some("main".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    // Fast-forward merge (just update master to feature's commit)
    let feature_commit = Branch::find_branch_result("feature", None)
        .await
        .expect("failed to query feature branch")
        .expect("feature branch should exist")
        .commit;
    Branch::update_branch("main", &feature_commit.to_string(), None)
        .await
        .unwrap();

    // Now try -d again (should succeed - fully merged)
    execute(BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: Some("feature".to_string()),
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    })
    .await;

    // Feature branch should be deleted
    assert!(
        Branch::find_branch_result("feature", None)
            .await
            .expect("failed to query feature branch")
            .is_none()
    );
}

/// Scenario: comprehensive coverage of `--contains` and `--no-contains`
/// filter semantics over a divergent branch topology:
///
/// ```text
///   master:  base ← m1 ← m2
///             ↖
///   dev:        d1 ← d2
/// ```
///
/// Where:
/// - `base`: common ancestor, reachable from both branches
/// - `m1`, `m2`: commits unique to master
/// - `d1`, `d2`: commits unique to dev (d1 branches from base, d2 extends d1)
///
/// Tests cover:
/// 1. Single filters (`--contains` or `--no-contains` alone)
/// 2. Combined filters (`--contains` AND `--no-contains`)
/// 3. Multiple values (OR semantics for `--contains`, AND for `--no-contains`)
/// 4. Chain dependency edge cases (e.g. `--contains d1 --no-contains d2`
///    is empty because d2 contains d1).
///
/// The `libra/intent` agent branch is filtered out before assertions to
/// keep the expected sets clean.
#[tokio::test]
#[serial]
async fn test_branch_contains_commit_filter() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    let main_branch = match Head::current().await {
        Head::Branch(name) => name,
        _ => panic!("expected to start on a branch"),
    };

    let make_commit = |msg: &str| CommitArgs {
        message: Some(msg.to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };

    // ================================================================
    //  Build commit graph: divergent branches with shared ancestor
    // ================================================================

    // Common ancestor
    commit::execute(make_commit("base")).await;
    let base = Head::current_commit().await.unwrap().to_string();

    // Create dev branch and add two commits
    execute(BranchArgs {
        new_branch: Some("dev".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    })
    .await;

    switch::execute(SwitchArgs {
        branch: Some("dev".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    commit::execute(make_commit("d1")).await;
    let d1 = Head::current_commit().await.unwrap().to_string();

    commit::execute(make_commit("d2")).await;
    let d2 = Head::current_commit().await.unwrap().to_string();

    // Return to main branch and add two commits
    switch::execute(SwitchArgs {
        branch: Some(main_branch.clone()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    commit::execute(make_commit("m1")).await;
    let m1 = Head::current_commit().await.unwrap().to_string();

    commit::execute(make_commit("m2")).await;
    let m2 = Head::current_commit().await.unwrap().to_string();

    // -- Helper: resolve commits from `&[String]` to `HashSet<ObjectHash>`
    let resolve_commits = async |commits: &[String]| {
        let mut set = HashSet::new();
        for commit in commits {
            let target_commit = match get_target_commit(commit).await {
                Ok(commit) => commit,
                Err(e) => panic!("fatal: {e}"),
            };
            set.insert(target_commit);
        }
        set
    };

    // -- Helper: filter and return sorted branch names --
    let run_filter = |contains: &[&str], no_contains: &[&str]| {
        let contains: Vec<String> = contains.iter().map(|s| s.to_string()).collect();
        let no_contains: Vec<String> = no_contains.iter().map(|s| s.to_string()).collect();
        async move {
            let mut branches = Branch::list_branches_result(None)
                .await
                .expect("failed to list branches");
            branches.retain(|b| b.name != "libra/intent");
            filter_branches(
                &mut branches,
                &resolve_commits(&contains).await,
                &resolve_commits(&no_contains).await,
            )
            .unwrap();
            let mut names: Vec<String> = branches.into_iter().map(|b| b.name).collect();
            names.sort();
            names
        }
    };

    let sorted = |names: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        v.sort();
        v
    };

    // ================================================================
    //  Test single `--contains` filter
    // ================================================================

    // Common ancestor is in both branches
    assert_eq!(
        run_filter(&[&base], &[]).await,
        sorted(&[&main_branch, "dev"]),
        "`--contains base` should match both branches"
    );

    // Branch-specific commits
    assert_eq!(
        run_filter(&[&d1], &[]).await,
        sorted(&["dev"]),
        "`--contains d1` should match only dev"
    );

    assert_eq!(
        run_filter(&[&d2], &[]).await,
        sorted(&["dev"]),
        "`--contains d2` (tip of dev) should match only dev"
    );

    assert_eq!(
        run_filter(&[&m1], &[]).await,
        sorted(&[&main_branch]),
        "`--contains m1` should match only master"
    );

    assert_eq!(
        run_filter(&[&m2], &[]).await,
        sorted(&[&main_branch]),
        "`--contains m2` (tip of master) should match only master"
    );

    // ================================================================
    //  Test single `--no-contains` filter
    // ================================================================

    // Excluding common ancestor filters out everything
    assert_eq!(
        run_filter(&[], &[&base]).await,
        sorted(&[]),
        "`--no-contains base` should match nothing"
    );

    // Excluding branch-specific commits
    assert_eq!(
        run_filter(&[], &[&d1]).await,
        sorted(&[&main_branch]),
        "`--no-contains d1` should match only master"
    );

    assert_eq!(
        run_filter(&[], &[&m1]).await,
        sorted(&["dev"]),
        "`--no-contains m1` should match only dev"
    );

    // ================================================================
    //  Test multiple `--contains` (OR semantics)
    // ================================================================

    // Any branch containing d1 OR m1
    assert_eq!(
        run_filter(&[&d1, &m1], &[]).await,
        sorted(&[&main_branch, "dev"]),
        "`--contains d1 --contains m1` should match both (OR)"
    );

    // Any branch containing d2 OR m2 (both tips)
    assert_eq!(
        run_filter(&[&d2, &m2], &[]).await,
        sorted(&[&main_branch, "dev"]),
        "`--contains d2 --contains m2` should match both (OR)"
    );

    // ================================================================
    //  Test multiple `--no-contains` (AND semantics)
    // ================================================================

    // Branches excluding both d1 AND m1 → none (each branch has one)
    assert_eq!(
        run_filter(&[], &[&d1, &m1]).await,
        sorted(&[]),
        "`--no-contains d1 --no-contains m1` should match nothing (each branch has one)"
    );

    // ================================================================
    //  Test combined `--contains` and `--no-contains`
    // ================================================================

    // Branches with base but not m1 → dev
    assert_eq!(
        run_filter(&[&base], &[&m1]).await,
        sorted(&["dev"]),
        "`--contains base --no-contains m1` should match dev"
    );

    // Branches with base but not d1 → master
    assert_eq!(
        run_filter(&[&base], &[&d1]).await,
        sorted(&[&main_branch]),
        "`--contains base --no-contains d1` should match master"
    );

    // Branches with base but not m2 → dev
    assert_eq!(
        run_filter(&[&base], &[&m2]).await,
        sorted(&["dev"]),
        "`--contains base --no-contains m2` should match dev"
    );

    // Branches with d1 OR m1, but not d2 → only master (dev is excluded by d2)
    assert_eq!(
        run_filter(&[&d1, &m1], &[&d2]).await,
        sorted(&[&main_branch]),
        "`--contains d1 --contains m1 --no-contains d2` should match master"
    );

    // Branches with d1 OR m1, but not m2 → only dev (master is excluded by m2)
    assert_eq!(
        run_filter(&[&d1, &m1], &[&m2]).await,
        sorted(&["dev"]),
        "`--contains d1 --contains m1 --no-contains m2` should match dev"
    );

    // ================================================================
    //  Test edge cases
    // ================================================================

    // Chain dependency: d2 contains d1, so `--contains d1 --no-contains d2` → empty
    assert_eq!(
        run_filter(&[&d1], &[&d2]).await,
        sorted(&[]),
        "`--contains d1 --no-contains d2` should match nothing (d2 contains d1)"
    );

    // Similarly for master chain
    assert_eq!(
        run_filter(&[&m1], &[&m2]).await,
        sorted(&[]),
        "`--contains m1 --no-contains m2` should match nothing (m2 contains m1)"
    );

    // Branches with base but excluding both tips → none
    assert_eq!(
        run_filter(&[&base], &[&d2, &m2]).await,
        sorted(&[]),
        "`--contains base --no-contains d2 --no-contains m2` should match nothing"
    );
}

/// Scenario: `filter_branches` must propagate (not swallow) errors when
/// a branch row points at a non-existent commit hash. The BFS inside
/// `commit_contains` should fail to load the bogus commit, and the
/// outer call must surface that error with a "failed to load commit"
/// message. Regression guard for silent-skip bugs.
#[test]
#[serial]
fn test_filter_branches_propagates_error_for_corrupt_commit() {
    use std::str::FromStr;

    use git_internal::hash::ObjectHash;
    use libra::internal::branch::Branch;

    let temp_path = tempdir().unwrap();
    init_repo_via_cli(temp_path.path());
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Fabricate a branch whose commit hash does not exist in any storage.
    let bogus_hash =
        ObjectHash::from_str("0000000000000000000000000000000000000000000000000000000000000000")
            .expect("valid hex");
    let corrupt_branch = Branch {
        name: "corrupt".into(),
        commit: bogus_hash,
        remote: None,
    };

    // `contains_set` with a real-looking hash forces BFS traversal.
    let mut branches = vec![corrupt_branch];
    let mut contains = HashSet::new();
    contains.insert(
        ObjectHash::from_str("1111111111111111111111111111111111111111111111111111111111111111")
            .expect("valid hex"),
    );
    let no_contains = HashSet::new();

    let result = filter_branches(&mut branches, &contains, &no_contains);
    assert!(
        result.is_err(),
        "filter_branches should propagate error for corrupt commit, got Ok"
    );
    let err = result.unwrap_err();
    assert!(
        err.message().contains("failed to load commit"),
        "error should mention failed commit load, got: {}",
        err.message()
    );
}

// =====================================================================
//  Wave 1 — `--merged` / `--no-merged` / `--points-at` / `--ignore-case`
// =====================================================================

/// Collect branch names from `libra branch` human output in printed order,
/// stripping the `* ` current-branch marker and surrounding whitespace.
fn list_names_in_order(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim_start)
        .filter(|line| !line.is_empty() && !line.starts_with("HEAD detached"))
        .map(|line| line.trim_start_matches("* ").trim().to_string())
        .collect()
}

/// Scenario: end-to-end `--merged` / `--no-merged` / `--points-at` over a
/// divergent graph built through the real CLI:
///
/// ```text
/// main:           base ← m1                (HEAD)
/// merged-topic:   base                     (ancestor of m1 → merged)
/// unmerged-topic: base ← m1 ← u1           (not an ancestor of m1)
/// ```
///
/// `--merged` (baseline HEAD = m1) keeps branches whose tip is reachable
/// from m1; `--no-merged` keeps the complement; `--points-at HEAD` keeps
/// only the exact-tip match; and `--merged --contains HEAD` proves the two
/// filters combine with AND semantics.
#[test]
#[serial]
fn test_branch_merged_no_merged_points_at_cli() {
    let repo = create_committed_repo_via_cli();

    // merged-topic stays pinned at the base commit.
    assert_cli_success(
        &run_libra_command(&["branch", "merged-topic"], repo.path()),
        "create merged-topic",
    );
    // Advance main to m1.
    assert_cli_success(
        &run_libra_command(
            &["commit", "--allow-empty", "-m", "m1", "--no-verify"],
            repo.path(),
        ),
        "commit m1 on main",
    );
    // unmerged-topic forks at m1 and gains its own commit u1.
    assert_cli_success(
        &run_libra_command(&["branch", "unmerged-topic"], repo.path()),
        "create unmerged-topic",
    );
    assert_cli_success(
        &run_libra_command(&["switch", "unmerged-topic"], repo.path()),
        "switch to unmerged-topic",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "--allow-empty", "-m", "u1", "--no-verify"],
            repo.path(),
        ),
        "commit u1 on unmerged-topic",
    );
    assert_cli_success(
        &run_libra_command(&["switch", "main"], repo.path()),
        "switch back to main",
    );

    // --merged (default baseline HEAD = m1)
    let merged = run_libra_command(&["branch", "--merged"], repo.path());
    assert_cli_success(&merged, "branch --merged");
    let names = list_names_in_order(&String::from_utf8_lossy(&merged.stdout));
    assert!(
        names.iter().any(|n| n == "main"),
        "merged should list main: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "merged-topic"),
        "merged should list merged-topic: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "unmerged-topic"),
        "merged must exclude unmerged-topic: {names:?}"
    );

    // --no-merged is the exact complement.
    let no_merged = run_libra_command(&["branch", "--no-merged"], repo.path());
    assert_cli_success(&no_merged, "branch --no-merged");
    let names = list_names_in_order(&String::from_utf8_lossy(&no_merged.stdout));
    assert!(
        names.iter().any(|n| n == "unmerged-topic"),
        "no-merged should list unmerged-topic: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "main"),
        "no-merged must exclude main: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "merged-topic"),
        "no-merged must exclude merged-topic: {names:?}"
    );

    // --points-at HEAD matches only the current tip (m1).
    let points = run_libra_command(&["branch", "--points-at", "HEAD"], repo.path());
    assert_cli_success(&points, "branch --points-at HEAD");
    let names = list_names_in_order(&String::from_utf8_lossy(&points.stdout));
    assert_eq!(
        names,
        vec!["main".to_string()],
        "points-at HEAD should match only main: {names:?}"
    );

    // --merged AND --contains HEAD narrows {main, merged-topic} to {main}
    // (merged-topic does not contain m1; unmerged-topic is not merged).
    let combined = run_libra_command(&["branch", "--merged", "--contains", "HEAD"], repo.path());
    assert_cli_success(&combined, "branch --merged --contains HEAD");
    let names = list_names_in_order(&String::from_utf8_lossy(&combined.stdout));
    assert_eq!(
        names,
        vec!["main".to_string()],
        "merged AND contains-HEAD should match only main: {names:?}"
    );
}

/// `--merged` and `--no-merged` are mutually exclusive. The clap conflict is
/// surfaced through libra's parse-error path as `CliInvalidArguments`
/// (`LBR-CLI-002`), which is coarse exit code 129 (the fine-grained `2` only
/// applies under `LIBRA_FINE_EXIT_CODES=1`).
#[test]
#[serial]
fn test_branch_merged_and_no_merged_conflict_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["branch", "--merged", "--no-merged"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        stderr.contains("cannot be used with")
            && stderr.contains("--merged")
            && stderr.contains("--no-merged"),
        "conflicting --merged/--no-merged should be rejected at parse time: {stderr}"
    );
}

/// In detached-HEAD mode the default `--merged` baseline resolves to the
/// detached commit (not an error), and branches reachable from it are listed.
#[test]
#[serial]
fn test_branch_merged_detached_head_uses_detached_commit() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "topic"], repo.path()),
        "create topic at base",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "--allow-empty", "-m", "m1", "--no-verify"],
            repo.path(),
        ),
        "advance main to m1",
    );
    assert_cli_success(
        &run_libra_command(&["switch", "--detach", "HEAD"], repo.path()),
        "detach HEAD at m1",
    );

    let merged = run_libra_command(&["branch", "--merged"], repo.path());
    assert_cli_success(&merged, "branch --merged while detached");
    let names = list_names_in_order(&String::from_utf8_lossy(&merged.stdout));
    assert!(
        names.iter().any(|n| n == "main"),
        "detached --merged should list main (reachable from detached commit): {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "topic"),
        "detached --merged should list topic (base is an ancestor of m1): {names:?}"
    );
}

/// `--ignore-case` switches the list sort from case-sensitive (ASCII, so
/// uppercase sorts before lowercase) to case-insensitive ordering.
#[test]
#[serial]
fn test_branch_ignore_case_sorts_case_insensitively() {
    let repo = create_committed_repo_via_cli();
    for name in ["Zebra", "apple", "Banana"] {
        assert_cli_success(
            &run_libra_command(&["branch", name], repo.path()),
            "create mixed-case branch",
        );
    }

    // Case-sensitive default: 'B'(66) < 'Z'(90) < 'a'(97).
    let sensitive = run_libra_command(&["branch"], repo.path());
    assert_cli_success(&sensitive, "branch (case-sensitive list)");
    let names = list_names_in_order(&String::from_utf8_lossy(&sensitive.stdout));
    let after_main: Vec<&String> = names.iter().filter(|n| *n != "main").collect();
    assert_eq!(
        after_main,
        vec!["Banana", "Zebra", "apple"],
        "default sort is case-sensitive: {names:?}"
    );

    // Case-insensitive: apple < Banana < Zebra.
    let folded = run_libra_command(&["branch", "--ignore-case"], repo.path());
    assert_cli_success(&folded, "branch --ignore-case");
    let names = list_names_in_order(&String::from_utf8_lossy(&folded.stdout));
    let after_main: Vec<&String> = names.iter().filter(|n| *n != "main").collect();
    assert_eq!(
        after_main,
        vec!["apple", "Banana", "Zebra"],
        "--ignore-case folds case before sorting: {names:?}"
    );
}

/// A branch name longer than the 255-byte ref limit is rejected by the
/// validator with `LBR-CLI-002` and no branch is created.
#[test]
#[serial]
fn test_branch_overlong_name_rejected() {
    let repo = create_committed_repo_via_cli();
    let overlong = "x".repeat(256);
    let output = run_libra_command(&["branch", &overlong], repo.path());
    assert!(
        !output.status.success(),
        "256-byte branch name should be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("is not a valid branch name"),
        "overlong name should report an invalid-name message, got: {stderr}"
    );
    assert!(
        stderr.contains("Error-Code: LBR-CLI-002"),
        "overlong name should surface CliInvalidArguments (LBR-CLI-002), got: {stderr}"
    );
}

/// Unit boundary check for the 255-byte limit added in Wave 1: exactly 255
/// bytes is accepted, 256 bytes is rejected.
#[test]
fn test_is_valid_git_branch_name_length_limit() {
    use libra::command::branch::is_valid_git_branch_name;

    assert!(
        is_valid_git_branch_name(&"a".repeat(255)),
        "255-byte name should be valid"
    );
    assert!(
        !is_valid_git_branch_name(&"a".repeat(256)),
        "256-byte name should be rejected"
    );
}

/// Unit coverage for the pure filter helpers backing `--merged` /
/// `--no-merged` (set membership) and `--points-at` (exact-tip match).
#[test]
#[serial]
fn test_branch_filter_helpers_are_pure() {
    use std::str::FromStr;

    use libra::internal::branch::{Branch, retain_by_merge_status, retain_by_points_at};

    // ObjectHash::from_str needs the repo's hash kind initialised.
    let temp_path = tempdir().unwrap();
    init_repo_via_cli(temp_path.path());
    let _guard = ChangeDirGuard::new(temp_path.path());

    let hash = |c: char| ObjectHash::from_str(&c.to_string().repeat(64)).expect("valid hex");
    let merged_tip = hash('1');
    let unmerged_tip = hash('2');
    let baseline_anc = hash('3');

    let reachable: HashSet<ObjectHash> = [merged_tip, baseline_anc].into_iter().collect();
    let names =
        |branches: &[Branch]| -> Vec<String> { branches.iter().map(|b| b.name.clone()).collect() };

    // want_merged = true keeps only the tip inside the reachable set.
    let mut keep_merged = vec![
        Branch {
            name: "merged".into(),
            commit: merged_tip,
            remote: None,
        },
        Branch {
            name: "unmerged".into(),
            commit: unmerged_tip,
            remote: None,
        },
    ];
    retain_by_merge_status(&mut keep_merged, &reachable, true);
    assert_eq!(names(&keep_merged), vec!["merged".to_string()]);

    // want_merged = false keeps the complement.
    let mut keep_unmerged = vec![
        Branch {
            name: "merged".into(),
            commit: merged_tip,
            remote: None,
        },
        Branch {
            name: "unmerged".into(),
            commit: unmerged_tip,
            remote: None,
        },
    ];
    retain_by_merge_status(&mut keep_unmerged, &reachable, false);
    assert_eq!(names(&keep_unmerged), vec!["unmerged".to_string()]);

    // points-at keeps only the exact-tip match.
    let mut points = vec![
        Branch {
            name: "merged".into(),
            commit: merged_tip,
            remote: None,
        },
        Branch {
            name: "unmerged".into(),
            commit: unmerged_tip,
            remote: None,
        },
    ];
    retain_by_points_at(&mut points, &merged_tip);
    assert_eq!(names(&points), vec!["merged".to_string()]);
}

/// `--json branch -l` must emit the list envelope (`{"ok","command":"branch",
/// "data":{"action":"list","branches":[...]}}`) with a stable per-branch
/// `commit` field, and must never leak the human-only skip-serialized fields
/// (`ignore_case`/`head_name`/`detached_head`/`show_unborn_head`/`display_name`).
/// Pins the JSON list contract that Wave 1 extended with the `ignore_case`
/// field — a regression that un-skipped it or renamed `commit` would otherwise
/// pass the whole suite.
#[test]
#[serial]
fn test_branch_json_list_exposes_commit_field() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "topic"], repo.path()),
        "create topic",
    );

    let output = run_libra_command(&["--json", "branch", "-l"], repo.path());
    assert_cli_success(&output, "--json branch -l");
    let json = parse_json_stdout(&output);

    assert_eq!(json["command"], "branch");
    assert_eq!(json["data"]["action"], "list");
    let branches = json["data"]["branches"]
        .as_array()
        .expect("data.branches must be an array");
    assert!(!branches.is_empty(), "list should not be empty");
    for entry in branches {
        assert!(
            entry["name"].as_str().is_some(),
            "entry.name must be a string"
        );
        assert!(
            entry["commit"].as_str().is_some(),
            "entry.commit must be a string"
        );
        assert!(
            entry["current"].as_bool().is_some(),
            "entry.current must be a bool"
        );
        // display_name is a human-only field and must stay out of JSON.
        assert!(
            entry.get("display_name").is_none(),
            "display_name must not serialize"
        );
    }
    // Human-only List fields must stay skip-serialized.
    for key in [
        "ignore_case",
        "head_name",
        "detached_head",
        "show_unborn_head",
    ] {
        assert!(
            json["data"].get(key).is_none(),
            "{key} must not appear in the list JSON envelope"
        );
    }

    // --ignore-case must not leak into the envelope either.
    let folded = run_libra_command(&["--json", "branch", "-l", "--ignore-case"], repo.path());
    assert_cli_success(&folded, "--json branch -l --ignore-case");
    let folded_json = parse_json_stdout(&folded);
    assert!(
        folded_json["data"].get("ignore_case").is_none(),
        "ignore_case must not leak into JSON even when set"
    );
}

/// A bare `libra branch` (no action flag) defaults to the list action and
/// produces the same branch set as `branch -l`; the JSON envelope reports
/// `data.action == "list"`.
#[test]
#[serial]
fn test_branch_default_no_action_is_list() {
    let repo = create_committed_repo_via_cli();
    for name in ["alpha", "beta"] {
        assert_cli_success(
            &run_libra_command(&["branch", name], repo.path()),
            "create branch",
        );
    }

    let bare = run_libra_command(&["branch"], repo.path());
    assert_cli_success(&bare, "bare branch lists");
    let listed = run_libra_command(&["branch", "-l"], repo.path());
    assert_cli_success(&listed, "branch -l lists");

    let mut bare_names = list_names_in_order(&String::from_utf8_lossy(&bare.stdout));
    let mut l_names = list_names_in_order(&String::from_utf8_lossy(&listed.stdout));
    bare_names.sort();
    l_names.sort();
    assert_eq!(bare_names, l_names, "bare `branch` must equal `branch -l`");
    assert!(bare_names.iter().any(|n| n == "alpha"));
    assert!(bare_names.iter().any(|n| n == "beta"));
    assert!(bare_names.iter().any(|n| n == "main"));

    // Bare invocation routes to the list action in JSON.
    let json = run_libra_command(&["--json", "branch"], repo.path());
    assert_cli_success(&json, "--json bare branch");
    let v = parse_json_stdout(&json);
    assert_eq!(
        v["data"]["action"], "list",
        "bare branch JSON action must be list"
    );
}

// =====================================================================
//  Wave 2 — `--unset-upstream` + tracking display in list
// =====================================================================

/// Build a `BranchArgs` for an `--unset-upstream` invocation with all other
/// fields defaulted. `Some(None)` targets the current branch; `Some(Some(n))`
/// targets the named branch.
fn unset_upstream_args(unset_upstream: Option<Option<String>>) -> BranchArgs {
    BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    }
}

/// `--unset-upstream` (current branch) removes BOTH `branch.<name>.remote`
/// and `branch.<name>.merge` from config_kv.
#[tokio::test]
#[serial]
async fn test_branch_unset_upstream_removes_both_keys() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    let branch = match Head::current().await {
        Head::Branch(name) => name,
        _ => panic!("expected to start on a branch"),
    };
    ConfigKv::set(&format!("branch.{branch}.remote"), "origin", false)
        .await
        .expect("set remote");
    ConfigKv::set(&format!("branch.{branch}.merge"), "refs/heads/main", false)
        .await
        .expect("set merge");

    execute(unset_upstream_args(Some(None))).await;

    assert!(
        ConfigKv::get(&format!("branch.{branch}.remote"))
            .await
            .expect("get remote")
            .is_none(),
        "branch.<name>.remote must be removed"
    );
    assert!(
        ConfigKv::get(&format!("branch.{branch}.merge"))
            .await
            .expect("get merge")
            .is_none(),
        "branch.<name>.merge must be removed"
    );
}

/// `--unset-upstream <branch>` clears tracking for a NAMED (non-current)
/// branch's config.
#[tokio::test]
#[serial]
async fn test_branch_unset_upstream_named() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    ConfigKv::set("branch.feature.remote", "origin", false)
        .await
        .expect("set remote");
    ConfigKv::set("branch.feature.merge", "refs/heads/feature", false)
        .await
        .expect("set merge");

    execute(unset_upstream_args(Some(Some("feature".to_string())))).await;

    assert!(
        ConfigKv::get("branch.feature.remote")
            .await
            .expect("get remote")
            .is_none(),
        "named branch remote must be removed"
    );
    assert!(
        ConfigKv::get("branch.feature.merge")
            .await
            .expect("get merge")
            .is_none(),
        "named branch merge must be removed"
    );
}

/// `--unset-upstream` on a branch with no tracking is an idempotent no-op:
/// it completes successfully and leaves config untouched.
#[tokio::test]
#[serial]
async fn test_branch_unset_upstream_noop_when_absent() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    // No tracking configured; must not panic or create keys.
    execute(unset_upstream_args(Some(Some("ghost".to_string())))).await;

    assert!(
        ConfigKv::get("branch.ghost.remote")
            .await
            .expect("get remote")
            .is_none()
    );
    assert!(
        ConfigKv::get("branch.ghost.merge")
            .await
            .expect("get merge")
            .is_none()
    );
}

/// `--unset-upstream` from detached HEAD without an explicit branch name
/// reports `LBR-REPO-003` (exit 128) and writes nothing.
#[test]
#[serial]
fn test_branch_unset_upstream_detached_errors() {
    let repo = create_committed_repo_via_cli();
    let detach = run_libra_command(&["switch", "--detach", "HEAD"], repo.path());
    assert!(
        detach.status.success(),
        "detach failed: {}",
        String::from_utf8_lossy(&detach.stderr)
    );

    let output = run_libra_command(&["branch", "--unset-upstream"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(stderr.contains("HEAD is detached"));
}

/// `branch -l` renders a `[<remote>/<branch>]` tracking suffix; after
/// `--unset-upstream` the suffix disappears. End-to-end via the binary.
#[test]
#[serial]
fn test_branch_list_shows_tracking_suffix_then_unset_clears_it() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/repo.git",
            ],
            repo.path(),
        ),
        "remote add origin",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "--set-upstream-to", "origin/main"], repo.path()),
        "set upstream",
    );

    let listed = run_libra_command(&["branch", "-l"], repo.path());
    assert_cli_success(&listed, "branch -l with tracking");
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        stdout.contains("[origin/main]"),
        "list should show tracking suffix: {stdout}"
    );

    let unset = run_libra_command(&["branch", "--unset-upstream"], repo.path());
    assert_cli_success(&unset, "unset upstream");
    let stdout = String::from_utf8_lossy(&unset.stdout);
    assert!(
        stdout.contains("Removed upstream tracking"),
        "unset should confirm removal: {stdout}"
    );

    let relisted = run_libra_command(&["branch", "-l"], repo.path());
    assert_cli_success(&relisted, "branch -l after unset");
    let stdout = String::from_utf8_lossy(&relisted.stdout);
    assert!(
        !stdout.contains("[origin/main]"),
        "tracking suffix must be gone after unset: {stdout}"
    );
}

/// JSON list output: untracked branches carry NO `tracking` key (backward
/// compatible); a tracked branch carries `tracking: {remote, merge}`.
#[test]
#[serial]
fn test_branch_list_json_tracking_backward_compatible() {
    let repo = create_committed_repo_via_cli();

    // Untracked: no `tracking` key anywhere.
    let untracked = run_libra_command(&["--json", "branch", "-l"], repo.path());
    assert_cli_success(&untracked, "--json branch -l untracked");
    let json = parse_json_stdout(&untracked);
    for entry in json["data"]["branches"].as_array().expect("branches array") {
        assert!(
            entry.get("tracking").is_none(),
            "untracked entry must omit tracking: {entry}"
        );
    }

    // Configure tracking, then the entry must expose tracking.{remote,merge}.
    assert_cli_success(
        &run_libra_command(
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/repo.git",
            ],
            repo.path(),
        ),
        "remote add origin",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "--set-upstream-to", "origin/main"], repo.path()),
        "set upstream",
    );

    let tracked = run_libra_command(&["--json", "branch", "-l"], repo.path());
    assert_cli_success(&tracked, "--json branch -l tracked");
    let json = parse_json_stdout(&tracked);
    let main_entry = json["data"]["branches"]
        .as_array()
        .expect("branches array")
        .iter()
        .find(|e| e["name"] == "main")
        .expect("main entry");
    assert_eq!(main_entry["tracking"]["remote"], "origin");
    assert_eq!(main_entry["tracking"]["merge"], "main");
}

/// `--json branch --unset-upstream` emits the unset-upstream envelope with the
/// exact `data` key set {action, branch, had_upstream}; the no-op case reports
/// `had_upstream=false` and exits 0.
#[test]
#[serial]
fn test_branch_unset_upstream_json_schema() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/repo.git",
            ],
            repo.path(),
        ),
        "remote add origin",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "--set-upstream-to", "origin/main"], repo.path()),
        "set upstream",
    );

    let output = run_libra_command(&["--json", "branch", "--unset-upstream"], repo.path());
    assert_cli_success(&output, "--json unset-upstream");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "branch");
    assert_eq!(json["data"]["action"], "unset-upstream");
    assert_eq!(json["data"]["branch"], "main");
    assert_eq!(json["data"]["had_upstream"], true);

    // Second invocation is a no-op: had_upstream=false, still exit 0.
    let noop = run_libra_command(&["--json", "branch", "--unset-upstream"], repo.path());
    assert_cli_success(&noop, "--json unset-upstream noop");
    let json = parse_json_stdout(&noop);
    assert_eq!(json["data"]["action"], "unset-upstream");
    assert_eq!(json["data"]["had_upstream"], false);
}

/// Listing a branch that tracks a now-deleted remote must not crash: the
/// configured tracking name is still shown (config read is independent of
/// remote existence).
#[test]
#[serial]
fn test_branch_list_survives_dangling_remote() {
    let repo = create_committed_repo_via_cli();
    // Write tracking config directly, pointing at a remote that does not exist.
    assert_cli_success(
        &run_libra_command(&["config", "branch.main.remote", "ghost"], repo.path()),
        "set branch.main.remote",
    );
    assert_cli_success(
        &run_libra_command(
            &["config", "branch.main.merge", "refs/heads/main"],
            repo.path(),
        ),
        "set branch.main.merge",
    );

    let listed = run_libra_command(&["branch", "-l"], repo.path());
    assert_cli_success(&listed, "branch -l with dangling remote");
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        stdout.contains("[ghost/main]"),
        "list should show tracking name even for a missing remote: {stdout}"
    );
}

// =====================================================================
//  Wave 3 — copy (-c/-C), txn-hardened rename/copy, reflog migration,
//           -f/--force, --create-reflog (accepted-noop)
// =====================================================================

/// A fully-defaulted `BranchArgs` (list mode, every action off) for tests that
/// override one field via struct-update syntax.
fn branch_args_default() -> BranchArgs {
    BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        unset_upstream: None,
        show_current: false,
        rename: vec![],
        copy: vec![],
        force_copy: vec![],
        edit_description: None,
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
        merged: None,
        no_merged: None,
        points_at: None,
        ignore_case: false,
        sort: None,
        format: None,
        force: false,
        create_reflog: false,
        track: None,
        no_track: false,
    }
}

/// Read the `commit` field of a named branch from `--json branch -l`.
fn branch_commit_json(repo: &std::path::Path, name: &str) -> Option<String> {
    let out = run_libra_command(&["--json", "branch", "-l"], repo);
    let json = parse_json_stdout(&out);
    json["data"]["branches"]
        .as_array()?
        .iter()
        .find(|e| e["name"] == name)?
        .get("commit")?
        .as_str()
        .map(str::to_string)
}

/// `branch -c old new` creates a destination pointing at the source tip.
#[test]
#[serial]
fn test_branch_copy_creates_pointer_to_same_commit() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "old"], repo.path()),
        "create old",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "-c", "old", "newcopy"], repo.path()),
        "copy old -> newcopy",
    );
    let old_commit = branch_commit_json(repo.path(), "old").expect("old commit");
    let new_commit = branch_commit_json(repo.path(), "newcopy").expect("newcopy commit");
    assert_eq!(old_commit, new_commit, "copy must point at the same commit");
}

/// `branch -C old existing` overwrites an existing destination; plain `-c`
/// without force is refused and leaves the destination untouched.
#[test]
#[serial]
fn test_branch_copy_force_overwrites_existing() {
    let repo = create_committed_repo_via_cli();
    // old @ base, then advance main so `another` (created now) differs from old.
    assert_cli_success(
        &run_libra_command(&["branch", "old"], repo.path()),
        "create old",
    );
    assert_cli_success(
        &run_libra_command(
            &["commit", "--allow-empty", "-m", "m1", "--no-verify"],
            repo.path(),
        ),
        "advance main",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "existing"], repo.path()),
        "create existing at m1",
    );
    let old_commit = branch_commit_json(repo.path(), "old").expect("old commit");

    // Without force: refused, `existing` unchanged.
    let plain = run_libra_command(&["branch", "-c", "old", "existing"], repo.path());
    assert!(
        !plain.status.success(),
        "plain -c must refuse existing dest"
    );
    let still = branch_commit_json(repo.path(), "existing").expect("existing commit");
    assert_ne!(still, old_commit, "rejected copy must not mutate existing");

    // With -C: overwrite to old's commit.
    assert_cli_success(
        &run_libra_command(&["branch", "-C", "old", "existing"], repo.path()),
        "force-copy old -> existing",
    );
    let now = branch_commit_json(repo.path(), "existing").expect("existing commit");
    assert_eq!(
        now, old_commit,
        "force-copy must reset existing to old's tip"
    );
}

/// Copy mirrors the source's tracking + description config onto the
/// destination.
#[tokio::test]
#[serial]
async fn test_branch_copy_migrates_config_tracking() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    // Give an empty commit so a branch tip exists, then create `src`.
    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: true,
        author: None,
    })
    .await;
    execute(BranchArgs {
        new_branch: Some("src".to_string()),
        ..branch_args_default()
    })
    .await;
    ConfigKv::set("branch.src.remote", "origin", false)
        .await
        .expect("set remote");
    ConfigKv::set("branch.src.merge", "refs/heads/main", false)
        .await
        .expect("set merge");
    ConfigKv::set("branch.src.description", "the source branch", false)
        .await
        .expect("set description");

    execute(BranchArgs {
        copy: vec!["src".to_string(), "dst".to_string()],
        ..branch_args_default()
    })
    .await;

    assert_eq!(
        ConfigKv::get("branch.dst.remote")
            .await
            .unwrap()
            .map(|e| e.value),
        Some("origin".to_string())
    );
    assert_eq!(
        ConfigKv::get("branch.dst.merge")
            .await
            .unwrap()
            .map(|e| e.value),
        Some("refs/heads/main".to_string())
    );
    assert_eq!(
        ConfigKv::get("branch.dst.description")
            .await
            .unwrap()
            .map(|e| e.value),
        Some("the source branch".to_string())
    );
    // Source config is preserved (copy, not move).
    assert!(
        ConfigKv::get("branch.src.remote").await.unwrap().is_some(),
        "copy must not remove source config"
    );
}

/// Copy migrates the source ref's reflog faithfully (field-for-field) when it
/// has entries; with no source reflog the copy is a clean no-op.
#[tokio::test]
#[serial]
async fn test_branch_copy_migrates_reflog_when_present() {
    use libra::internal::reflog::{Reflog, ReflogAction, ReflogContext};

    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: true,
        author: None,
    })
    .await;
    execute(BranchArgs {
        new_branch: Some("src".to_string()),
        ..branch_args_default()
    })
    .await;

    let db = libra::internal::db::get_db_conn_instance().await;
    // Seed two reflog rows on the source ref.
    for i in 0..2 {
        Reflog::insert_single_entry(
            &db,
            &ReflogContext {
                old_oid: format!("{i}{}", "0".repeat(63)),
                new_oid: format!("{}{}", i + 1, "0".repeat(63)),
                action: ReflogAction::Commit {
                    message: format!("entry {i}"),
                },
            },
            "refs/heads/src",
        )
        .await
        .expect("seed reflog");
    }
    let src_entries = Reflog::find_all(&db, "refs/heads/src")
        .await
        .expect("src reflog");
    assert_eq!(src_entries.len(), 2);

    execute(BranchArgs {
        copy: vec!["src".to_string(), "dst".to_string()],
        ..branch_args_default()
    })
    .await;

    let dst_entries = Reflog::find_all(&db, "refs/heads/dst")
        .await
        .expect("dst reflog");
    assert_eq!(dst_entries.len(), 2, "all reflog rows must migrate");
    // Faithful field-for-field copy. The two seeded rows can share a timestamp,
    // so compare as order-independent tuple sets rather than zipping by index.
    let fingerprint = |e: &libra::internal::model::reflog::Model| {
        (
            e.old_oid.clone(),
            e.new_oid.clone(),
            e.action.clone(),
            e.committer_name.clone(),
            e.committer_email.clone(),
            e.timestamp,
            e.message.clone(),
        )
    };
    let mut src_fp: Vec<_> = src_entries.iter().map(fingerprint).collect();
    let mut dst_fp: Vec<_> = dst_entries.iter().map(fingerprint).collect();
    src_fp.sort();
    dst_fp.sort();
    assert_eq!(src_fp, dst_fp, "reflog rows must be copied field-for-field");
    for d in &dst_entries {
        assert_eq!(d.ref_name, "refs/heads/dst", "ref_name must be rewritten");
    }
}

/// Copy with no source reflog leaves the destination reflog empty (no error,
/// no assumption that the source has a reflog).
#[tokio::test]
#[serial]
async fn test_branch_copy_no_reflog_is_noop() {
    use libra::internal::reflog::Reflog;

    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: true,
        author: None,
    })
    .await;
    execute(BranchArgs {
        new_branch: Some("src".to_string()),
        ..branch_args_default()
    })
    .await;
    execute(BranchArgs {
        copy: vec!["src".to_string(), "dst".to_string()],
        ..branch_args_default()
    })
    .await;

    let db = libra::internal::db::get_db_conn_instance().await;
    assert!(
        Reflog::find_all(&db, "refs/heads/dst")
            .await
            .expect("dst reflog")
            .is_empty(),
        "no source reflog => empty destination reflog"
    );
}

/// Locked branches: rejected as rename source and as copy/force-copy target,
/// but allowed as a read-only copy source.
#[test]
#[serial]
fn test_branch_locked_protection_copy_rename() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "topic"], repo.path()),
        "create topic",
    );

    // (a) rename a locked source -> Locked (128).
    let rename_locked = run_libra_command(&["branch", "-m", "main", "renamed"], repo.path());
    let (_s, report) = parse_cli_error_stderr(&rename_locked.stderr);
    assert_eq!(rename_locked.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");

    // (b) copy onto a locked target -> Locked (128).
    let copy_to_locked = run_libra_command(&["branch", "-c", "topic", "main"], repo.path());
    assert_eq!(copy_to_locked.status.code(), Some(128));
    let (_s, report) = parse_cli_error_stderr(&copy_to_locked.stderr);
    assert_eq!(report.error_code, "LBR-CONFLICT-002");

    // (b') force-copy onto a locked target is STILL refused.
    let force_to_locked = run_libra_command(&["branch", "-C", "topic", "main"], repo.path());
    assert_eq!(force_to_locked.status.code(), Some(128));

    // (c) copy FROM a locked source to a normal target -> success.
    let copy_from_locked = run_libra_command(&["branch", "-c", "main", "from-main"], repo.path());
    assert_cli_success(&copy_from_locked, "copy from locked source");
    let main_commit = branch_commit_json(repo.path(), "main").expect("main commit");
    let copy_commit = branch_commit_json(repo.path(), "from-main").expect("from-main commit");
    assert_eq!(
        main_commit, copy_commit,
        "copy-from-locked must duplicate tip"
    );
}

/// `branch -m` (rename) refuses a locked source.
#[test]
#[serial]
fn test_branch_rename_locked_source_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["branch", "-m", "main", "trunk"], repo.path());
    assert_eq!(output.status.code(), Some(128));
    let (_s, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
}

/// Renaming the current branch migrates all config keys, updates HEAD in the
/// same transaction, and leaves no `branch.<old>.*` residue.
#[tokio::test]
#[serial]
async fn test_branch_rename_migrates_config_and_updates_head() {
    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: true,
        author: None,
    })
    .await;
    // Create + switch to a renamable (non-locked) current branch.
    execute(BranchArgs {
        new_branch: Some("feature".to_string()),
        ..branch_args_default()
    })
    .await;
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;
    ConfigKv::set("branch.feature.remote", "origin", false)
        .await
        .expect("set remote");
    ConfigKv::set("branch.feature.description", "desc", false)
        .await
        .expect("set description");

    execute(BranchArgs {
        rename: vec!["feature".to_string(), "feature2".to_string()],
        ..branch_args_default()
    })
    .await;

    // HEAD followed the rename.
    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "feature2", "HEAD must follow rename"),
        _ => panic!("expected to be on a branch"),
    }
    // Config migrated, no residue.
    assert_eq!(
        ConfigKv::get("branch.feature2.remote")
            .await
            .unwrap()
            .map(|e| e.value),
        Some("origin".to_string())
    );
    assert_eq!(
        ConfigKv::get("branch.feature2.description")
            .await
            .unwrap()
            .map(|e| e.value),
        Some("desc".to_string())
    );
    assert!(
        ConfigKv::get("branch.feature.remote")
            .await
            .unwrap()
            .is_none(),
        "old branch config must be removed"
    );
    assert!(
        ConfigKv::get("branch.feature.description")
            .await
            .unwrap()
            .is_none(),
        "old branch description must be removed"
    );
}

/// `branch -f <existing>` resets an existing branch's tip instead of erroring.
#[test]
#[serial]
fn test_branch_create_force_resets_existing() {
    let repo = create_committed_repo_via_cli();
    let base = branch_commit_json(repo.path(), "main").expect("main commit");
    assert_cli_success(
        &run_libra_command(&["branch", "topic"], repo.path()),
        "create topic at base",
    );
    // Advance main so HEAD differs from topic.
    assert_cli_success(
        &run_libra_command(
            &["commit", "--allow-empty", "-m", "m1", "--no-verify"],
            repo.path(),
        ),
        "advance main",
    );

    // Plain create on existing -> AlreadyExists.
    let plain = run_libra_command(&["branch", "topic"], repo.path());
    assert!(
        !plain.status.success(),
        "plain create on existing must fail"
    );

    // -f resets topic to current HEAD (m1).
    assert_cli_success(
        &run_libra_command(&["branch", "-f", "topic"], repo.path()),
        "force-create resets topic",
    );
    let head = branch_commit_json(repo.path(), "main").expect("main commit");
    let topic = branch_commit_json(repo.path(), "topic").expect("topic commit");
    assert_eq!(topic, head, "force-create must reset topic to HEAD");
    assert_ne!(
        topic, base,
        "topic should no longer point at the base commit"
    );
}

/// `--create-reflog` is accepted and is a strict no-op (branch still created,
/// exit 0).
#[test]
#[serial]
fn test_branch_create_reflog_flag_accepted_noop() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["branch", "feature", "--create-reflog"], repo.path());
    assert_cli_success(&output, "branch --create-reflog");
    // Branch was created normally.
    assert!(
        branch_commit_json(repo.path(), "feature").is_some(),
        "branch should be created with --create-reflog"
    );
}

/// `-c old new --json` emits the copy envelope with the exact `data` key set
/// {action, src, dst, commit, force}.
#[test]
#[serial]
fn test_branch_copy_json_schema() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "old"], repo.path()),
        "create old",
    );
    let output = run_libra_command(&["--json", "branch", "-c", "old", "newcopy"], repo.path());
    assert_cli_success(&output, "--json copy");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "branch");
    assert_eq!(json["data"]["action"], "copy");
    assert_eq!(json["data"]["src"], "old");
    assert_eq!(json["data"]["dst"], "newcopy");
    assert!(json["data"]["commit"].as_str().is_some());
    assert_eq!(json["data"]["force"], false);

    // Force-copy reports force=true.
    let forced = run_libra_command(&["--json", "branch", "-C", "old", "newcopy"], repo.path());
    assert_cli_success(&forced, "--json force copy");
    let json = parse_json_stdout(&forced);
    assert_eq!(json["data"]["force"], true);
}

// =====================================================================
//  Wave 4 — `--edit-description` + list description metadata
// =====================================================================

/// Write an executable stub editor script and return its path. The stub
/// receives the temp file path as `$1`. Unix-only (the editor is invoked via
/// `sh -c`).
#[cfg(unix)]
fn write_stub_editor(dir: &std::path::Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    fs::write(&path, body).expect("write stub editor");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod stub editor");
    path.to_str().expect("utf8 path").to_string()
}

/// `--edit-description` with no configured editor (`EDITOR=""`, no
/// `core.editor`) is a usage error: `LBR-CLI-002`, exit 129, and it must not
/// hang.
#[test]
#[serial]
fn test_branch_edit_description_no_editor_errors() {
    let repo = create_committed_repo_via_cli();
    // Target a non-locked branch so the editor check (not the locked guard) is
    // what's exercised.
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], repo.path()),
        "create feature",
    );
    let output = run_libra_command_with_stdin_and_env(
        &["branch", "--edit-description", "feature"],
        repo.path(),
        "",
        &[("EDITOR", "")],
    );
    assert_eq!(output.status.code(), Some(129));
    let (_s, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// `--edit-description` refuses a locked branch (before touching the editor):
/// `LBR-CONFLICT-002`, exit 128.
#[test]
#[serial]
fn test_branch_edit_description_locked_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command_with_stdin_and_env(
        &["branch", "--edit-description", "main"],
        repo.path(),
        "",
        &[("EDITOR", "")],
    );
    assert_eq!(output.status.code(), Some(128));
    let (_s, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
}

/// `--edit-description` from detached HEAD without a branch name reports
/// `LBR-REPO-003` (exit 128).
#[test]
#[serial]
fn test_branch_edit_description_detached_errors() {
    let repo = create_committed_repo_via_cli();
    let detach = run_libra_command(&["switch", "--detach", "HEAD"], repo.path());
    assert!(detach.status.success(), "detach failed");
    let output = run_libra_command_with_stdin_and_env(
        &["branch", "--edit-description"],
        repo.path(),
        "",
        &[("EDITOR", "")],
    );
    assert_eq!(output.status.code(), Some(128));
    let (_s, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
}

/// `--edit-description` on a non-locked branch captures the editor's content,
/// persists it to `branch.<name>.description`, and the `--json` envelope
/// reports `{action, branch, description}`.
#[cfg(unix)]
#[test]
#[serial]
fn test_branch_edit_description_writes_and_persists() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], repo.path()),
        "create feature",
    );
    let editor = write_stub_editor(
        repo.path(),
        "ed.sh",
        "#!/bin/sh\nprintf 'the feature branch\\n' > \"$1\"\n",
    );

    let output = run_libra_command_with_stdin_and_env(
        &["--json", "branch", "--edit-description", "feature"],
        repo.path(),
        "",
        &[("EDITOR", editor.as_str())],
    );
    assert_cli_success(&output, "edit-description feature");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "branch");
    assert_eq!(json["data"]["action"], "edit-description");
    assert_eq!(json["data"]["branch"], "feature");
    assert_eq!(json["data"]["description"], "the feature branch");

    // Persisted: list JSON exposes the description on the feature entry.
    let listed = run_libra_command(&["--json", "branch", "-l"], repo.path());
    let json = parse_json_stdout(&listed);
    let feature = json["data"]["branches"]
        .as_array()
        .expect("branches")
        .iter()
        .find(|e| e["name"] == "feature")
        .expect("feature entry");
    assert_eq!(feature["description"], "the feature branch");
}

/// A stub editor that exits non-zero is treated as a cancel: the description is
/// left unchanged and the command exits 0.
#[cfg(unix)]
#[test]
#[serial]
fn test_branch_edit_description_nonzero_exit_is_cancel() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], repo.path()),
        "create feature",
    );
    // Pre-set a description via config.
    assert_cli_success(
        &run_libra_command(
            &["config", "branch.feature.description", "original"],
            repo.path(),
        ),
        "set description",
    );
    let editor = write_stub_editor(repo.path(), "cancel.sh", "#!/bin/sh\nexit 1\n");

    let output = run_libra_command_with_stdin_and_env(
        &["branch", "--edit-description", "feature"],
        repo.path(),
        "",
        &[("EDITOR", editor.as_str())],
    );
    assert_cli_success(&output, "edit-description cancel exits 0");

    // Description unchanged.
    let listed = run_libra_command(&["--json", "branch", "-l"], repo.path());
    let json = parse_json_stdout(&listed);
    let feature = json["data"]["branches"]
        .as_array()
        .expect("branches")
        .iter()
        .find(|e| e["name"] == "feature")
        .expect("feature entry");
    assert_eq!(
        feature["description"], "original",
        "cancel must not change description"
    );
}

/// Editing to an empty description clears `branch.<name>.description`; the list
/// entry then omits the field (backward compatible).
#[cfg(unix)]
#[test]
#[serial]
fn test_branch_edit_description_empty_clears() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], repo.path()),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(
            &["config", "branch.feature.description", "to be cleared"],
            repo.path(),
        ),
        "set description",
    );
    let editor = write_stub_editor(repo.path(), "empty.sh", "#!/bin/sh\n: > \"$1\"\n");

    let output = run_libra_command_with_stdin_and_env(
        &["--json", "branch", "--edit-description", "feature"],
        repo.path(),
        "",
        &[("EDITOR", editor.as_str())],
    );
    assert_cli_success(&output, "edit-description empty clears");
    let json = parse_json_stdout(&output);
    // Cleared description is omitted from the JSON (skip_serializing_if).
    assert!(
        json["data"].get("description").is_none(),
        "cleared description must be omitted from JSON"
    );

    let listed = run_libra_command(&["--json", "branch", "-l"], repo.path());
    let json = parse_json_stdout(&listed);
    let feature = json["data"]["branches"]
        .as_array()
        .expect("branches")
        .iter()
        .find(|e| e["name"] == "feature")
        .expect("feature entry");
    assert!(
        feature.get("description").is_none(),
        "list entry must omit cleared description"
    );
}

/// UTF-8 (multi-byte) descriptions round-trip losslessly through the editor.
#[cfg(unix)]
#[test]
#[serial]
fn test_branch_edit_description_utf8_roundtrip() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], repo.path()),
        "create feature",
    );
    let editor = write_stub_editor(
        repo.path(),
        "utf8.sh",
        "#!/bin/sh\nprintf '%s\\n' '\u{63cf}\u{8ff0} \u{1f33f}' > \"$1\"\n",
    );

    let output = run_libra_command_with_stdin_and_env(
        &["--json", "branch", "--edit-description", "feature"],
        repo.path(),
        "",
        &[("EDITOR", editor.as_str())],
    );
    assert_cli_success(&output, "edit-description utf8");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["description"], "\u{63cf}\u{8ff0} \u{1f33f}");
}

/// Setting a description via config surfaces it in `branch -l --json`; branches
/// without a description omit the field (backward compatible).
#[test]
#[serial]
fn test_branch_list_json_exposes_description() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], repo.path()),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(
            &["config", "branch.feature.description", "documented branch"],
            repo.path(),
        ),
        "set description",
    );

    let listed = run_libra_command(&["--json", "branch", "-l"], repo.path());
    assert_cli_success(&listed, "branch -l --json");
    let json = parse_json_stdout(&listed);
    let branches = json["data"]["branches"].as_array().expect("branches");
    let feature = branches
        .iter()
        .find(|e| e["name"] == "feature")
        .expect("feature entry");
    assert_eq!(feature["description"], "documented branch");
    // main has no description -> field omitted.
    let main = branches
        .iter()
        .find(|e| e["name"] == "main")
        .expect("main entry");
    assert!(
        main.get("description").is_none(),
        "branch without a description must omit the field"
    );
}

// =====================================================================
//  Wave 5 — decline --track/--no-track, --sort/--format stub,
//           name-length boundary
// =====================================================================

/// `--track[=mode]` is declined as a usage error (`LBR-CLI-002`, exit 129)
/// with a hint pointing at the supported alternatives — both the `=mode` form
/// and the bare flag.
#[test]
#[serial]
fn test_branch_track_declined_exit_129() {
    let repo = create_committed_repo_via_cli();
    for args in [
        vec!["branch", "--track=direct", "newbranch"],
        vec!["branch", "--track"],
    ] {
        let output = run_libra_command(&args, repo.path());
        assert_eq!(output.status.code(), Some(129), "args: {args:?}");
        let (stderr, report) = parse_cli_error_stderr(&output.stderr);
        assert_eq!(report.error_code, "LBR-CLI-002", "args: {args:?}");
        assert!(
            stderr.contains("switch --track") || stderr.contains("branch -u"),
            "decline hint should point at the alternatives: {stderr}"
        );
    }
    // The declined create must not have produced a branch.
    let listed = run_libra_command(&["branch", "-l"], repo.path());
    let names = list_names_in_order(&String::from_utf8_lossy(&listed.stdout));
    assert!(
        !names.iter().any(|n| n == "newbranch"),
        "declined --track must not create the branch: {names:?}"
    );
}

/// `--no-track` is declined the same way (exit 129, `LBR-CLI-002`).
#[test]
#[serial]
fn test_branch_no_track_declined_exit_129() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["branch", "--no-track", "newbranch"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    let (_s, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    let listed = run_libra_command(&["branch", "-l"], repo.path());
    let names = list_names_in_order(&String::from_utf8_lossy(&listed.stdout));
    assert!(!names.iter().any(|n| n == "newbranch"));
}

/// `--track` declined error is JSON-renderable with its stable code under
/// `--json` (still exit 129).
#[test]
#[serial]
fn test_branch_track_declined_json_stable_code() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--json", "branch", "--track=inherit"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    let (_s, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// `--sort` is accepted but ignored on the list path: the list still prints in
/// the default order, a `note:` is emitted to stderr, and the exit code is 0.
#[test]
#[serial]
fn test_branch_sort_flag_stub_exit0_with_note() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "topic"], repo.path()),
        "create topic",
    );
    let output = run_libra_command(&["branch", "--sort=committerdate"], repo.path());
    assert!(output.status.success(), "stub --sort must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let names = list_names_in_order(&stdout);
    assert!(
        names.iter().any(|n| n == "main"),
        "list still printed: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "topic"),
        "list still printed: {names:?}"
    );
    assert!(
        stderr.contains("note:") && stderr.contains("--json"),
        "stderr should carry the informational note: {stderr}"
    );
}

/// `--format` behaves like `--sort`: ignored, default list, stderr note, exit 0.
#[test]
#[serial]
fn test_branch_format_flag_stub_exit0_with_note() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["branch", "--format=%(refname)"], repo.path());
    assert!(output.status.success(), "stub --format must exit 0");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("note:") && stderr.contains("--json"),
        "stderr should carry the informational note: {stderr}"
    );
    let names = list_names_in_order(&String::from_utf8_lossy(&output.stdout));
    assert!(names.iter().any(|n| n == "main"));
}

/// `--sort` is in the `query` group, so combining it with a create positional
/// trips the action↔query mutual exclusion. Libra surfaces that clap conflict
/// as a usage error (exit 129, like other branch parse conflicts) and the
/// branch is NOT created.
#[test]
#[serial]
fn test_branch_sort_with_create_is_group_conflict() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["branch", "newbranch", "--sort=committerdate"],
        repo.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "create + query-group flag must be rejected as a usage conflict"
    );
    let listed = run_libra_command(&["branch", "-l"], repo.path());
    let names = list_names_in_order(&String::from_utf8_lossy(&listed.stdout));
    assert!(
        !names.iter().any(|n| n == "newbranch"),
        "conflicting invocation must not create the branch: {names:?}"
    );
}

/// Branch-name validation stays lenient: camelCase and multi-segment dotted
/// names are valid; only the existing illegal forms plus the 255-byte cap are
/// rejected.
#[test]
fn test_branch_valid_name_allows_camelcase_and_dotted() {
    use libra::command::branch::is_valid_git_branch_name;
    assert!(is_valid_git_branch_name("featureBranch"));
    assert!(is_valid_git_branch_name("feature/sub_topic.v2"));
    assert!(is_valid_git_branch_name("release/2.0-rc1"));
    // A bare '@' (not '@{') and interior '/' remain legal.
    assert!(is_valid_git_branch_name("user@host-work"));
    // Still rejects the existing illegal forms.
    assert!(!is_valid_git_branch_name("bad..name"));
    assert!(!is_valid_git_branch_name("has space"));
    assert!(!is_valid_git_branch_name("ends/"));
}

/// camelCase / dotted names are creatable end-to-end through the CLI.
#[test]
#[serial]
fn test_branch_create_camelcase_and_dotted_via_cli() {
    let repo = create_committed_repo_via_cli();
    for name in ["featureBranch", "feature/sub_topic.v2"] {
        assert_cli_success(
            &run_libra_command(&["branch", name], repo.path()),
            "create lenient name",
        );
    }
    let listed = run_libra_command(&["branch", "-l"], repo.path());
    let names = list_names_in_order(&String::from_utf8_lossy(&listed.stdout));
    assert!(names.iter().any(|n| n == "featureBranch"));
    assert!(names.iter().any(|n| n == "feature/sub_topic.v2"));
}
