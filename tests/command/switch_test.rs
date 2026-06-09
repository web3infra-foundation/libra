//! Tests switch command for branch creation, switching, and dirty-state checks.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use clap::Parser;
use git_internal::internal::index::Index;
use libra::{
    internal::{
        branch::{AGENT_TRACES_BRANCH, Branch as InternalBranch},
        head::Head,
    },
    utils::{client_storage::ClientStorage, path, test::ChangeDirGuard},
};

use super::*;

#[test]
fn test_switch_cli_missing_branch_returns_cli_exit_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["switch", "no-such"], repo.path());

    assert_eq!(output.status.code(), Some(129));
    assert!(String::from_utf8_lossy(&output.stderr).contains("branch 'no-such' not found"));
}

/// opencode.md OC-Phase 3 acceptance criterion 5 requires that
/// `switch` refuse to create a branch named `intent` or
/// `agent-traces`. The runtime guard at
/// `src/command/switch.rs::is_locked_branch` covers both, but the
/// `switch_test` suite previously had no coverage at all for the
/// locked-name refusal — a regression that dropped the guard could
/// have shipped silently.
#[test]
fn test_switch_create_intent_branch_is_blocked() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["switch", "-c", "intent"], repo.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("intent"),
        "expected the locked branch name in the message, got: {stderr}"
    );
}

/// Companion to `test_switch_create_intent_branch_is_blocked` for the
/// `agent-traces` locked name. Without the guard, `switch -c
/// agent-traces` could shadow the reserved capture ref locally and
/// then propagate via `push`.
#[test]
fn test_switch_create_agent_traces_branch_is_blocked() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["switch", "-c", "agent-traces"], repo.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("agent-traces"),
        "expected the agent-traces branch name in the message, got: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_switch_existing_agent_traces_branch_is_blocked() {
    let repo = create_committed_repo_via_cli();
    {
        let _guard = ChangeDirGuard::new(repo.path());
        let head = Head::current_commit()
            .await
            .expect("committed repo should have HEAD");
        InternalBranch::update_branch(AGENT_TRACES_BRANCH, &head.to_string(), None)
            .await
            .expect("seed agent-traces branch");
    }

    let output = run_libra_command(&["switch", AGENT_TRACES_BRANCH], repo.path());

    assert!(!output.status.success());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        stderr.contains(AGENT_TRACES_BRANCH),
        "expected the agent-traces branch name in the message, got: {stderr}"
    );
}

#[test]
fn test_switch_json_create_output_reports_new_branch() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "switch", "-c", "feature"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "switch");
    assert_eq!(json["data"]["branch"], "feature");
    assert_eq!(json["data"]["created"], true);
    assert_eq!(json["data"]["detached"], false);
}

#[tokio::test]
#[serial]
async fn test_switch_json_track_output_stays_clean() {
    let repo = create_committed_repo_via_cli();
    let remote = run_libra_command(
        &["remote", "add", "origin", "https://example.com/repo.git"],
        repo.path(),
    );
    assert_cli_success(&remote, "remote add origin");
    let _guard = ChangeDirGuard::new(repo.path());

    let head = Head::current_commit().await.unwrap();
    Branch::update_branch(
        "refs/remotes/origin/feature",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .unwrap();

    let output = run_libra_command(
        &["--json", "switch", "--track", "origin/feature"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "switch");
    assert_eq!(json["data"]["branch"], "feature");
    assert_eq!(json["data"]["tracking"]["remote"], "origin");
    assert_eq!(json["data"]["tracking"]["remote_branch"], "feature");
}

#[tokio::test]
#[serial]
async fn test_switch_track_human_output_keeps_tracking_message() {
    let repo = create_committed_repo_via_cli();
    let remote = run_libra_command(
        &["remote", "add", "origin", "https://example.com/repo.git"],
        repo.path(),
    );
    assert_cli_success(&remote, "remote add origin");
    let _guard = ChangeDirGuard::new(repo.path());

    let head = Head::current_commit().await.unwrap();
    Branch::update_branch(
        "refs/remotes/origin/feature",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .unwrap();

    let output = run_libra_command(&["switch", "--track", "origin/feature"], repo.path());
    assert_cli_success(&output, "switch --track");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Branch 'feature' set up to track remote branch 'origin/feature'"),
        "expected upstream tracking message in stdout, got: {stdout}"
    );
}

// async fn test_check_status() {
//     println!("\n\x1b[1mTest check_status function.\x1b[0m");
//
//     // Test the check_status
//     // Expect false when no changes
//     assert!(!check_status().await);
//
//     // Create a file and add it to the index
//     // Expect true when there are unstaged changes
//     fs::File::create("foo.txt").unwrap();
//     let add_args = add::AddArgs {
//         pathspec: vec!["foo.txt".to_string()],
//         all: false,
//         update: false,
//         verbose: true,
//         dry_run: false,
//         ignore_errors: false,
//         refresh: false,
//     };
//     add::execute(add_args).await;
//     assert!(check_status().await);
//
//     // Modify a file
//     // Expect true when there are uncommitted changes
//     fs::write("foo.txt", "modified content").unwrap();
//     assert!(check_status().await);
// }

async fn test_switch_function() {
    println!("\n\x1b[1mTest switch function.\x1b[0m");

    // create first empty commit
    {
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
            ..Default::default()
        };
        commit::execute(args).await;
    }

    // create a new branch and switch to it
    {
        let args = SwitchArgs {
            branch: None,
            create: Some("test_branch".to_string()),
            force_create: None,
            detach: false,
            track: false,
            ..Default::default()
        };
        switch::execute(args).await;
        let head = Head::current().await;
        let ref_name = match head {
            Head::Branch(name) => name,
            _ => panic!("head not in branch,unreachable"),
            // Head::Detached(name) => name.to_string(),
        };
        assert_eq!(
            ref_name, "test_branch",
            "create a new branch and switch to it failed!"
        );
    }

    //detach the head to a commit
    {
        let head = Head::current().await;
        let ref_name = match head {
            Head::Branch(name) => name,
            _ => panic!("head not in branch,unreachable"),
            // Head::Detached(name) => name.to_string(),
        };
        // Migrated from lossy `Branch::find_branch` per docs/improvement/branch.md.
        let branch = Branch::find_branch_result(&ref_name, None)
            .await
            .expect("failed to query current branch")
            .expect("current branch should exist");
        let commit: Commit = load_object(&branch.commit).unwrap();
        let commit_id_str = commit.id.to_string();

        let args = CommitArgs {
            message: Some("second".to_string()),
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
            ..Default::default()
        };
        commit::execute(args).await;

        let args = SwitchArgs {
            branch: Some(commit_id_str.clone()),
            create: None,
            force_create: None,
            detach: true,
            track: false,
            ..Default::default()
        };
        switch::execute(args).await;
        let head = Head::current().await;
        let ref_name = match head {
            Head::Detached(name) => name.to_string(),
            _ => panic!("head not detached,unreachable"),
            // Head::Detached(name) => name.to_string(),
        };
        println!("detach {ref_name:?}");
        assert_eq!(
            ref_name, commit_id_str,
            "detach the head to a commit failed!"
        );
    }

    //switch branch back to the master
    {
        let args = SwitchArgs {
            branch: Some("main".to_string()),
            create: None,
            force_create: None,
            detach: false,
            track: false,
            ..Default::default()
        };
        switch::execute(args).await;
        let head = Head::current().await;
        let ref_name = match head {
            Head::Branch(name) => name,
            _ => panic!("head not in branch,unreachable"),
            // Head::Detached(name) => name.to_string(),
        };
        assert_eq!(ref_name, "main", "switch back to the master failed!");
    }
}
#[tokio::test]
#[serial]
/// Tests the core functionality of the switch command module.
/// Validates branch switching operations and working directory status checks.
async fn test_parts_of_switch_module_function() {
    println!("\n\x1b[1mTest some functions of the switch module.\x1b[0m");
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    println!("temp_path {temp_path:?}");

    //Test check the branch
    test_switch_function().await;

    // Test the switch module funsctions
    // test_check_status().await;
}

#[test]
fn test_switch_current_branch_with_dirty_worktree_is_noop() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "modified content\n").unwrap();

    let output = run_libra_command(&["switch", "main"], repo.path());
    assert_cli_success(&output, "switch current branch");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Already on 'main'"),
        "switch current branch should remain a no-op, got: {stdout}"
    );
    assert!(
        !stdout.contains("Changes not staged") && !stdout.contains("On branch"),
        "switch current branch should not print a status summary, got: {stdout}"
    );
    let content = std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap();
    assert_eq!(content, "modified content\n");
}

#[test]
fn test_switch_create_branch_from_valid_commit() {
    let repo = create_committed_repo_via_cli();

    std::fs::write(repo.path().join("tracked.txt"), "tracked second\n").unwrap();
    let add = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add, "add tracked.txt");
    let commit = run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit second");

    let output = run_libra_command(&["switch", "-c", "feature-from-base", "HEAD^"], repo.path());
    assert_cli_success(&output, "switch -c feature-from-base HEAD^");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Switched to a new branch 'feature-from-base'"),
        "expected branch creation message, got: {stdout}"
    );

    let log_output = run_libra_command(&["log", "--oneline", "-1"], repo.path());
    assert_cli_success(&log_output, "log -1 after switch");
    let log_stdout = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        log_stdout.contains("base"),
        "expected new branch to point at the requested base commit, got: {log_stdout}"
    );

    let content = std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap();
    assert_eq!(content, "tracked\n");
}

#[tokio::test]
#[serial]
async fn test_switch_track_sets_upstream() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    let remote = run_libra_command(
        &["remote", "add", "origin", "https://example.com/repo.git"],
        temp_path.path(),
    );
    assert_cli_success(&remote, "remote add origin");

    let args = CommitArgs {
        message: Some("base".to_string()),
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
        ..Default::default()
    };
    commit::execute(args).await;

    let master_commit = Head::current_commit().await.unwrap();
    Branch::update_branch(
        "refs/remotes/origin/feature",
        &master_commit.to_string(),
        Some("origin"),
    )
    .await
    .unwrap();

    let args = SwitchArgs {
        branch: Some("origin/feature".to_string()),
        create: None,
        force_create: None,
        detach: false,
        track: true,
        ..Default::default()
    };
    switch::execute(args).await;

    let head = Head::current().await;
    let branch_name = match head {
        Head::Branch(name) => name,
        _ => panic!("head not in branch, unreachable"),
    };
    assert_eq!(branch_name, "feature");

    let branch_config = libra::internal::config::ConfigKv::branch_config("feature")
        .await
        .ok()
        .flatten()
        .unwrap();
    assert_eq!(branch_config.remote, "origin");
    assert_eq!(branch_config.merge, "feature");
}

#[tokio::test]
#[serial]
/// Tests basic HEAD detachment capabilities with simple reference paths.
/// Validates relative references (HEAD^, HEAD~), numeric references (HEAD^1, HEAD~1),
/// and complex reference combinations (HEAD^^^, HEAD~~~, HEAD^~^~).
async fn test_detach_head_basic() {
    println!("\n\x1b[1mTest detach use the head's ref basically.\x1b[0m");
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    println!("temp_path {temp_path:?}");

    for i in 0..6 {
        let args = CommitArgs {
            message: Some(format!("commit_{i}")),
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
            ..Default::default()
        };
        commit::execute(args).await;
    }
    //detach to head
    {
        switch_to_branch("main".to_string()).await;

        let commit_message = switch_to_detach("HEAD".to_string()).await;
        assert_eq!(&commit_message, "commit_5");
    }

    //detach to the before commit
    {
        let commit_message = switch_to_detach("HEAD^".to_string()).await;
        assert_eq!(&commit_message, "commit_4");
    }

    {
        let commit_message = switch_to_detach("HEAD~".to_string()).await;
        assert_eq!(&commit_message, "commit_3");
    }
    {
        let commit_message = switch_to_detach("HEAD^1".to_string()).await;
        assert_eq!(&commit_message, "commit_2");
    }

    {
        let commit_message = switch_to_detach("HEAD~1".to_string()).await;
        assert_eq!(&commit_message, "commit_1");
    }
    switch_to_branch("main".to_string()).await;

    for i in 6..12 {
        let args = CommitArgs {
            message: Some(format!("commit_{i}")),
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
            ..Default::default()
        };
        commit::execute(args).await;
    }

    //detach use head's ref
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach("HEAD~11".to_string()).await;
        assert_eq!(&commit_message, "commit_0");
    }
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach("HEAD~~~~~~~~~~~".to_string()).await;
        assert_eq!(&commit_message, "commit_0");
    }
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach("HEAD^^^^^^^^^^^".to_string()).await;
        assert_eq!(&commit_message, "commit_0");
    }

    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach("HEAD^~^~^~^~^~^".to_string()).await;
        assert_eq!(&commit_message, "commit_0");
    }
    //detach use branch's ref
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach("main~11".to_string()).await;
        assert_eq!(&commit_message, "commit_0");
    }
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach("main~~~~~~~~~~~".to_string()).await;
        assert_eq!(&commit_message, "commit_0");
    }
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach("main^^^^^^^^^^^".to_string()).await;
        assert_eq!(&commit_message, "commit_0");
    }

    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach("main^~^~^~^~^~^".to_string()).await;
        assert_eq!(&commit_message, "commit_0");
    }
    // Migrated from lossy `Branch::find_branch` per docs/improvement/branch.md.
    let master_commit_id = Branch::find_branch_result("main", None)
        .await
        .expect("failed to query main branch")
        .expect("main branch should exist")
        .commit;
    //detach use commit's ref
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach(format!("{master_commit_id}~11")).await;
        assert_eq!(&commit_message, "commit_0");
    }
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach(format!("{master_commit_id}~11")).await;
        assert_eq!(&commit_message, "commit_0");
    }
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach(format!("{master_commit_id}~~~~~~~~~~~")).await;
        assert_eq!(&commit_message, "commit_0");
    }
    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach(format!("{master_commit_id}^^^^^^^^^^^")).await;
        assert_eq!(&commit_message, "commit_0");
    }

    {
        switch_to_branch("main".to_string()).await;
        let commit_message = switch_to_detach(format!("{master_commit_id}^~^~^~^~^~^")).await;
        assert_eq!(&commit_message, "commit_0");
    }
}

// a tree with many parents.
async fn create_commit_tree() {
    let index = Index::load(path::index()).unwrap();
    let storage = ClientStorage::init(path::objects());

    let tree = commit::create_tree(&index, &storage, "".into())
        .await
        .unwrap();

    let mut commit_1 = Commit::from_tree_id(tree.id, vec![], &format_commit_msg("commit_0", None));
    commit_1.committer.timestamp = 1;
    save_object(&commit_1, &commit_1.id).unwrap();

    let mut parents_ids = vec![];
    for i in 1..12 {
        let tree = commit::create_tree(&index, &storage, "".into())
            .await
            .unwrap();

        let mut commit = Commit::from_tree_id(
            tree.id,
            vec![commit_1.id],
            &format_commit_msg(&format!("commit_{i}"), None),
        );
        commit.committer.timestamp = (i + 1) as usize;
        save_object(&commit, &commit.id).unwrap();
        parents_ids.push(commit.id);
    }
    {
        let tree = commit::create_tree(&index, &storage, "".into())
            .await
            .unwrap();

        let mut commit_last = Commit::from_tree_id(
            tree.id,
            parents_ids,
            &format_commit_msg("commit_last", None),
        );
        commit_last.committer.timestamp = 100;
        save_object(&commit_last, &commit_last.id).unwrap();
        Branch::update_branch("main", &commit_last.id.to_string(), None)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[serial]
// Comprehensive tests for HEAD reference navigation using Git-style paths
// Validates support for ^ (parent selection), ~ (ancestry traversal), and their combinations
async fn test_detach_head_extra() {
    println!("\n\x1b[1mTest detach use the head's ref extra.\x1b[0m");
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    println!("temp_path {temp_path:?}");

    create_commit_tree().await;
    //detach to head
    {
        let commit_message = switch_to_detach("HEAD".to_string()).await;
        assert_eq!(commit_message, "commit_last".to_string());
    }

    for i in 1..12 {
        let commit_message = switch_to_detach(format!("HEAD^{i}")).await;
        assert_eq!(commit_message, format!("commit_{i}"));

        //back to the last commit
        switch_to_branch("main".to_string()).await;
    }
    //detach use the branch's ref
    for i in 1..12 {
        let commit_message = switch_to_detach(format!("main^{i}")).await;
        assert_eq!(commit_message, format!("commit_{i}"));

        //back to the last commit
        switch_to_branch("main".to_string()).await;
    }
    //detach use head's ref
    {
        let commit_message = switch_to_detach("HEAD^11~".to_string()).await;
        assert_eq!(commit_message, "commit_0".to_string());
        switch_to_branch("main".to_string()).await;
    }
    //detach use branch's ref
    {
        let commit_message = switch_to_detach("main^11~".to_string()).await;
        assert_eq!(commit_message, "commit_0".to_string());
        switch_to_branch("main".to_string()).await;
    }
    // Migrated from lossy `Branch::find_branch` per docs/improvement/branch.md.
    let master_commit_id = Branch::find_branch_result("main", None)
        .await
        .expect("failed to query main branch")
        .expect("main branch should exist")
        .commit;
    //detach use commit's ref
    {
        let commit_message = switch_to_detach(format!("{master_commit_id}^11~")).await;
        assert_eq!(commit_message, "commit_0".to_string());
        switch_to_branch("main".to_string()).await;
    }
}

async fn switch_to_detach(branch_test: String) -> String {
    let args = SwitchArgs {
        branch: Some(branch_test),
        create: None,
        force_create: None,
        detach: true,
        track: false,
        ..Default::default()
    };
    switch::execute(args).await;
    let head = Head::current().await;
    let commit_id = match head {
        Head::Detached(commit) => commit,
        _ => panic!("head not detached,unreachable"),
    };
    let commit = load_object::<Commit>(&commit_id).unwrap();
    libra::common_utils::parse_commit_msg(&commit.message)
        .0
        .trim()
        .to_string()
}

async fn switch_to_branch(branch_test: String) {
    let args = SwitchArgs {
        branch: Some(branch_test),
        create: None,
        force_create: None,
        detach: false,
        track: false,
        ..Default::default()
    };
    switch::execute(args).await;
}

#[test]
#[serial]
fn test_switch_force_create_resets_existing_branch() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["switch", "-c", "feature"], repo.path()),
        "create feature",
    );
    assert_cli_success(
        &run_libra_command(&["switch", "main"], repo.path()),
        "back to main",
    );

    // `-c` on an existing branch is refused...
    let dup = run_libra_command(&["switch", "-c", "feature"], repo.path());
    assert!(
        !dup.status.success(),
        "switch -c on an existing branch must fail"
    );

    // ...but `-C` force-creates (resets) it and switches.
    let force = run_libra_command(&["switch", "-C", "feature"], repo.path());
    assert_cli_success(
        &force,
        "switch -C should reset and switch to an existing branch",
    );

    let current = run_libra_command(&["branch", "--show-current"], repo.path());
    assert_cli_success(&current, "branch --show-current");
    assert_eq!(
        String::from_utf8_lossy(&current.stdout).trim(),
        "feature",
        "should be on feature after switch -C"
    );
}

#[test]
fn switch_force_flags_parse_as_aliases() {
    for flag in ["-f", "--force", "--discard-changes"] {
        let args = SwitchArgs::try_parse_from(["switch", flag, "main"])
            .unwrap_or_else(|err| panic!("{flag} should parse: {err}"));
        assert_eq!(args.branch.as_deref(), Some("main"));
        assert!(args.force, "{flag} should enable force switching");
    }
}

#[test]
fn switch_guess_flags_parse_as_tristate() {
    let default_args = SwitchArgs::try_parse_from(["switch", "feature"]).unwrap();
    assert!(!default_args.guess);
    assert!(!default_args.no_guess);

    let guess_args = SwitchArgs::try_parse_from(["switch", "--guess", "feature"]).unwrap();
    assert!(guess_args.guess);
    assert!(!guess_args.no_guess);

    let no_guess_args = SwitchArgs::try_parse_from(["switch", "--no-guess", "feature"]).unwrap();
    assert!(!no_guess_args.guess);
    assert!(no_guess_args.no_guess);
}

#[test]
fn switch_mode_flags_are_mutually_exclusive() {
    for args in [
        vec!["switch", "-c", "one", "-C", "two"],
        vec!["switch", "-c", "one", "--orphan", "two"],
        vec!["switch", "-C", "one", "--detach", "main"],
        vec!["switch", "--orphan", "one", "--detach", "main"],
        vec!["switch", "--guess", "--no-guess", "feature"],
    ] {
        assert!(
            SwitchArgs::try_parse_from(args.clone()).is_err(),
            "{args:?} should be rejected by clap"
        );
    }
}

#[test]
fn switch_force_overwrites_dirty_tracked_file() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["branch", "other"], p), "branch other");
    assert_cli_success(&run_libra_command(&["switch", "other"], p), "switch other");
    std::fs::write(p.join("tracked.txt"), "target branch content\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "tracked.txt"], p), "add target");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "target edit", "--no-verify"], p),
        "commit target edit",
    );
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");

    std::fs::write(p.join("tracked.txt"), "dirty local edit\n").unwrap();
    let blocked = run_libra_command(&["switch", "other"], p);
    assert!(
        !blocked.status.success(),
        "plain switch should reject dirty tracked files"
    );

    assert_cli_success(
        &run_libra_command(&["switch", "-f", "other"], p),
        "switch -f",
    );
    assert_eq!(
        std::fs::read_to_string(p.join("tracked.txt")).unwrap(),
        "target branch content\n"
    );
}

#[test]
fn switch_force_still_rejects_untracked_overwrite() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["switch", "-c", "other"], p),
        "switch other",
    );
    std::fs::write(p.join("conflict.txt"), "tracked on target\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "conflict.txt"], p),
        "add conflict",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "target conflict", "--no-verify"], p),
        "commit target conflict",
    );
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");
    std::fs::write(p.join("conflict.txt"), "untracked local\n").unwrap();

    let output = run_libra_command(&["switch", "-f", "other"], p);
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("untracked working tree file would be overwritten"),
        "stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(p.join("conflict.txt")).unwrap(),
        "untracked local\n"
    );
}

#[tokio::test]
#[serial]
async fn switch_guess_creates_tracking_branch_by_default() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(
            &["remote", "add", "origin", "https://example.com/repo.git"],
            repo.path(),
        ),
        "remote add origin",
    );
    let _guard = ChangeDirGuard::new(repo.path());
    let head = Head::current_commit().await.unwrap();
    Branch::update_branch(
        "refs/remotes/origin/feature",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .unwrap();

    let output = run_libra_command(&["switch", "feature"], repo.path());
    assert_cli_success(&output, "switch feature should guess origin/feature");

    let branch_config = libra::internal::config::ConfigKv::branch_config("feature")
        .await
        .ok()
        .flatten()
        .expect("feature should have upstream config");
    assert_eq!(branch_config.remote, "origin");
    assert_eq!(branch_config.merge, "feature");
}

#[tokio::test]
#[serial]
async fn switch_no_guess_disables_remote_tracking_guess() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(
            &["remote", "add", "origin", "https://example.com/repo.git"],
            repo.path(),
        ),
        "remote add origin",
    );
    let _guard = ChangeDirGuard::new(repo.path());
    let head = Head::current_commit().await.unwrap();
    Branch::update_branch(
        "refs/remotes/origin/feature",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .unwrap();

    let output = run_libra_command(&["switch", "--no-guess", "feature"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("branch 'feature' not found"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[serial]
async fn switch_checkout_guess_false_disables_implicit_guess() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(
            &["remote", "add", "origin", "https://example.com/repo.git"],
            repo.path(),
        ),
        "remote add origin",
    );
    assert_cli_success(
        &run_libra_command(&["config", "checkout.guess", "false"], repo.path()),
        "config checkout.guess false",
    );
    let _guard = ChangeDirGuard::new(repo.path());
    let head = Head::current_commit().await.unwrap();
    Branch::update_branch(
        "refs/remotes/origin/feature",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .unwrap();

    let output = run_libra_command(&["switch", "feature"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("branch 'feature' not found"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
