//! Tests `libra bisect` for finding the commit that introduced a regression.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! Fixture convention: each test sets up a fresh repo via
//! `setup_with_new_libra_in()`, configures a stable identity, and uses
//! `create_linear_commits(n)` to lay down a straight chain of commits whose
//! hashes are returned newest-first. The sub-state machine `BisectState` is
//! inspected directly to verify that `start`/`bad`/`good`/`skip`/`reset`
//! transitions write the expected on-disk state. CLI-level smoke tests at
//! the bottom run the binary outside or inside an empty repo to confirm
//! the user-visible failure behaviour.

use std::process::Command;

use libra::{
    cli::Bisect,
    command::{
        add::{self, AddArgs},
        bisect::{BisectState, execute_safe},
        commit,
    },
    internal::{config::ConfigKv, head::Head},
    utils::{
        output::OutputConfig,
        test::{self, ChangeDirGuard},
    },
};
use serial_test::serial;
use tempfile::tempdir;

/// Run the Libra binary with an isolated HOME so host config never leaks into tests.
fn run_libra_command(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    std::fs::create_dir_all(&config_home).expect("failed to create isolated config directory");

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
        .env("LIBRA_TEST_ENV", "1")
        .output()
        .expect("failed to execute libra binary")
}

/// Initialize a repository through the CLI to exercise the real process entrypoint.
fn init_repo_via_cli(repo: &std::path::Path) {
    std::fs::create_dir_all(repo).expect("failed to create repository directory");
    let output = run_libra_command(&["init"], repo);
    assert!(
        output.status.success(),
        "failed to initialize repository: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Configure test identity directly through the in-process config layer.
/// Required before any commit because Libra refuses to author without it.
async fn configure_identity() {
    ConfigKv::set("user.name", "Bisect Test", false)
        .await
        .unwrap();
    ConfigKv::set("user.email", "bisect@test.com", false)
        .await
        .unwrap();
}

/// Create a linear chain of `count` commits, each modifying `file.txt`.
///
/// Returns the commit hashes ordered newest-first: `hashes[0]` is HEAD and
/// `hashes[count - 1]` is the root commit. The first commit also stages
/// `.libraignore` so subsequent runs see a clean tree. Assumes the caller
/// already holds a `ChangeDirGuard` rooted in a fresh repo.
async fn create_linear_commits(count: usize) -> Vec<String> {
    let mut hashes = Vec::new();

    for i in 0..count {
        test::ensure_file("file.txt", Some(&format!("content_{i}\n")));
        let pathspec = if i == 0 {
            vec![String::from(".libraignore"), String::from("file.txt")]
        } else {
            vec![String::from("file.txt")]
        };

        add::execute(AddArgs {
            pathspec,
            all: false,
            update: false,
            refresh: false,
            force: false,
            verbose: false,
            dry_run: false,
            ignore_errors: false,
        })
        .await;
        commit::execute(commit::CommitArgs {
            message: Some(format!("Commit {i}").to_string()),
            file: None,
            allow_empty: false,
            conventional: false,
            no_edit: false,
            amend: false,
            signoff: false,
            disable_pre: true,
            all: false,
            no_verify: false,
            author: None,
        })
        .await;

        let hash = Head::current_commit().await.unwrap().to_string();
        hashes.push(hash);
    }

    // Reverse so newest is first (hashes[0] = latest, hashes[n-1] = oldest)
    hashes.reverse();
    hashes
}

/// Scenario: `bisect start` (no bounds) must transition the repo into the
/// `in_progress` state with empty `bad` and `good` slots. Pins the initial
/// state shape.
#[tokio::test]
#[serial]
async fn test_bisect_start_creates_state() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    // Create at least one commit
    create_linear_commits(1).await;

    // Start bisect
    let args = Bisect::Start {
        bad: None,
        good: None,
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // Verify state was created
    assert!(BisectState::is_in_progress().await.unwrap());

    let state = BisectState::load().await.unwrap();
    assert!(state.bad.is_none());
    assert!(state.good.is_empty());
}

/// Scenario: `bisect start <bad> <good>` must record both bounds and
/// immediately check out a midpoint commit (`state.current` populated).
/// Confirms the binary search seeding behaviour.
#[tokio::test]
#[serial]
async fn test_bisect_start_with_bad_and_good() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    // Create 5 commits: hashes[0] = latest (Commit 4), hashes[4] = oldest (Commit 0)
    let hashes = create_linear_commits(5).await;

    // Start bisect with bad (latest) and good (oldest)
    let bad = hashes[0].clone(); // latest
    let good = hashes[4].clone(); // oldest

    let args = Bisect::Start {
        bad: Some(bad.clone()),
        good: Some(good.clone()),
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    let state = BisectState::load().await.unwrap();
    assert_eq!(state.bad.unwrap().to_string(), bad);
    assert_eq!(state.good[0].to_string(), good);

    // Should have checked out to a middle commit
    assert!(state.current.is_some());
}

/// Scenario: marking `bad` followed by `good` on a 3-commit chain narrows
/// the search to the single middle commit, which becomes `state.current`.
/// Locks in the bisection convergence path.
#[tokio::test]
#[serial]
async fn test_bisect_mark_bad_then_good() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    // Create 3 commits: hashes[0] = latest, hashes[2] = oldest
    let hashes = create_linear_commits(3).await;

    // Start bisect
    let args = Bisect::Start {
        bad: None,
        good: None,
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // Mark bad (latest)
    let bad = hashes[0].clone();
    let args = Bisect::Bad {
        rev: Some(bad.clone()),
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    let state = BisectState::load().await.unwrap();
    assert_eq!(state.bad.unwrap().to_string(), bad);

    // Mark good (oldest)
    let good = hashes[2].clone();
    let args = Bisect::Good { rev: Some(good) };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // Should now be on the middle commit (hashes[1])
    let state = BisectState::load().await.unwrap();
    assert_eq!(state.current.unwrap().to_string(), hashes[1]);
}

/// Scenario: end-to-end bisection over 7 commits where commits 4-6 are
/// "bad". The loop drives the algorithm to termination using the index of
/// the current commit as ground truth. Confirms the algorithm terminates
/// and exits the bisect session cleanly.
#[tokio::test]
#[serial]
async fn test_bisect_find_first_bad_commit() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    // Create 7 commits: hashes[0] = latest (Commit 6), hashes[6] = oldest (Commit 0)
    let hashes = create_linear_commits(7).await;

    // Start bisect with bad at Commit 6 (latest), good at Commit 3 (hashes[3])
    // So Commit 4, 5, 6 are bad, Commit 0, 1, 2, 3 are good
    // First bad commit should be hashes[3] (Commit 4 from user perspective, but index 3 in our array)
    let bad = hashes[0].clone(); // latest = Commit 6
    let good = hashes[3].clone(); // Commit 3

    let args = Bisect::Start {
        bad: Some(bad),
        good: Some(good),
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // Continue bisect until we find the first bad commit
    // The first bad commit should be hashes[2] (Commit 4 in sequence, which is index 2 from newest)
    loop {
        if !BisectState::is_in_progress().await.unwrap() {
            break;
        }

        let state = BisectState::load().await.unwrap();
        let current = state.current.unwrap().to_string();

        // For this test, commits 4, 5, 6 (hashes[0], [1], [2]) are bad
        // commits 0, 1, 2, 3 (hashes[3], [4], [5], [6]) are good
        let current_idx = hashes.iter().position(|h| h == &current).unwrap();

        if current_idx <= 2 {
            // This commit is bad (indices 0, 1, 2 are commits 6, 5, 4)
            let args = Bisect::Bad { rev: None };
            execute_safe(args, &OutputConfig::default()).await.unwrap();
        } else {
            // This commit is good
            let args = Bisect::Good { rev: None };
            execute_safe(args, &OutputConfig::default()).await.unwrap();
        }
    }

    // Bisect should have ended
    assert!(!BisectState::is_in_progress().await.unwrap());
}

/// Scenario: `bisect reset` must clear the in-progress state and return
/// HEAD to its pre-bisect commit. Pins both the state-cleanup and the
/// HEAD-restore behaviour after a session is started and `bad`/`good`
/// have moved HEAD off the original tip.
#[tokio::test]
#[serial]
async fn test_bisect_reset() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    let hashes = create_linear_commits(3).await;
    let orig_head = hashes[0].clone(); // latest

    // Start bisect
    let args = Bisect::Start {
        bad: None,
        good: None,
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // Mark commits
    let args = Bisect::Bad {
        rev: Some(hashes[0].clone()),
    }; // latest
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    let args = Bisect::Good {
        rev: Some(hashes[2].clone()),
    }; // oldest
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // Should be on middle commit
    let _state = BisectState::load().await.unwrap();
    assert_ne!(Head::current_commit().await.unwrap().to_string(), orig_head);

    // Reset
    let args = Bisect::Reset { rev: None };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // State should be cleared
    assert!(!BisectState::is_in_progress().await.unwrap());

    // Should be back to original HEAD
    assert_eq!(Head::current_commit().await.unwrap().to_string(), orig_head);
}

/// Scenario: `bisect skip` must record the current commit in
/// `state.skipped` and advance to a different commit. Locks in the skip
/// behaviour for untestable commits.
#[tokio::test]
#[serial]
async fn test_bisect_skip() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    // Create 5 commits
    let hashes = create_linear_commits(5).await;

    // Start bisect
    let bad = hashes[0].clone(); // latest
    let good = hashes[4].clone(); // oldest

    let args = Bisect::Start {
        bad: Some(bad),
        good: Some(good),
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    let state = BisectState::load().await.unwrap();
    let current = state.current.unwrap().to_string();

    // Skip current commit
    let args = Bisect::Skip { rev: None };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    let state = BisectState::load().await.unwrap();

    // Current should be skipped
    assert!(state.skipped.iter().any(|h| h.to_string() == current));

    // Should have moved to a different commit
    assert_ne!(state.current.unwrap().to_string(), current);
}

/// Scenario: `bisect log` must execute without error during an active
/// session. Smoke-tests the log subcommand path (the actual log content is
/// not asserted here).
#[tokio::test]
#[serial]
async fn test_bisect_log() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    let hashes = create_linear_commits(3).await;

    // Start bisect and mark some commits
    let args = Bisect::Start {
        bad: Some(hashes[0].clone()),
        good: Some(hashes[2].clone()),
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // Log should work
    let args = Bisect::Log;
    execute_safe(args, &OutputConfig::default()).await.unwrap();
}

/// Scenario: starting a second bisect session while one is active must
/// return an error. Pins the "single active session" invariant.
#[tokio::test]
#[serial]
async fn test_bisect_start_already_in_progress_fails() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    create_linear_commits(3).await;

    // Start first bisect
    let args = Bisect::Start {
        bad: None,
        good: None,
    };
    execute_safe(args, &OutputConfig::default()).await.unwrap();

    // Try to start again - should fail
    let args = Bisect::Start {
        bad: None,
        good: None,
    };
    let result = execute_safe(args, &OutputConfig::default()).await;
    assert!(result.is_err());
}

/// Scenario: `bad`, `good`, and `skip` must all return errors when no
/// bisect session has been started. Pins the no-implicit-session contract.
#[tokio::test]
#[serial]
async fn test_bisect_operations_without_session_fails() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    configure_identity().await;

    create_linear_commits(3).await;

    // Try bad without session
    let args = Bisect::Bad { rev: None };
    let result = execute_safe(args, &OutputConfig::default()).await;
    assert!(result.is_err());

    // Try good without session
    let args = Bisect::Good { rev: None };
    let result = execute_safe(args, &OutputConfig::default()).await;
    assert!(result.is_err());

    // Try skip without session
    let args = Bisect::Skip { rev: None };
    let result = execute_safe(args, &OutputConfig::default()).await;
    assert!(result.is_err());
}

/// Scenario: invoking `libra bisect start` outside any repo through the
/// real binary must exit 128 and emit a "fatal" message on stderr. Note
/// the explicit `#[::std::prelude::rust_2024::test]` path because the
/// surrounding async tests pull `tokio::test` into scope.
#[::std::prelude::rust_2024::test]
fn test_bisect_cli_outside_repository_returns_fatal() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(&["bisect", "start"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal"),
        "expected fatal error, got: {stderr}"
    );
}

/// Scenario: `libra bisect start` against a repo with no commits must
/// fail (no objects to walk). Captures the "empty history" error path.
#[::std::prelude::rust_2024::test]
fn test_bisect_cli_empty_repository_returns_fatal() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["bisect", "start"], repo.path());
    // Should fail because there are no commits
    assert!(!output.status.success());
}

// ── C4 surface tests: `bisect run` / `bisect view` ────────────────────────────────────────

/// `libra bisect --help` lists the new `run` and `view` subcommands plus
/// the EXAMPLES banner produced by `BISECT_EXAMPLES`.
#[::std::prelude::rust_2024::test]
fn test_bisect_help_lists_run_and_view() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["bisect", "--help"], repo.path());
    assert!(
        output.status.success(),
        "bisect --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("run"),
        "bisect --help should list 'run', stdout: {stdout}"
    );
    assert!(
        stdout.contains("view"),
        "bisect --help should list 'view', stdout: {stdout}"
    );
    assert!(
        stdout.contains("EXAMPLES:"),
        "bisect --help should include EXAMPLES, stdout: {stdout}"
    );
}

/// `bisect view` outside an active session must return `BisectNotActive`
/// (LBR-BISECT-001) so callers can distinguish "no bisect" from a transient
/// failure.
#[tokio::test]
#[serial]
async fn test_bisect_view_without_session_errors() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    configure_identity().await;
    let _hashes = create_linear_commits(3).await;

    let result = execute_safe(Bisect::View, &OutputConfig::default()).await;
    assert!(result.is_err(), "view without session must error");
    let err = result.unwrap_err();
    let stable = err.stable_code().as_str();
    assert_eq!(
        stable, "LBR-BISECT-001",
        "view without session must use BisectNotActive, got {stable}"
    );
}

/// `bisect view` during an active session prints state without erroring.
#[tokio::test]
#[serial]
async fn test_bisect_view_inside_active_session() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    configure_identity().await;
    let hashes = create_linear_commits(5).await;

    execute_safe(
        Bisect::Start {
            bad: Some(hashes[0].clone()),
            good: Some(hashes[4].clone()),
        },
        &OutputConfig::default(),
    )
    .await
    .unwrap();

    execute_safe(Bisect::View, &OutputConfig::default())
        .await
        .expect("view inside an active session must succeed");

    // Clean up.
    execute_safe(Bisect::Reset { rev: None }, &OutputConfig::default())
        .await
        .unwrap();
}

/// `bisect run` without an active session must reject with `BisectNotActive`.
/// The user must `bisect start` (with bounds) before automation kicks in.
#[tokio::test]
#[serial]
async fn test_bisect_run_without_session_errors() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    configure_identity().await;
    let _hashes = create_linear_commits(3).await;

    let result = execute_safe(
        Bisect::Run {
            cmd: vec!["true".to_string()],
        },
        &OutputConfig::default(),
    )
    .await;
    assert!(result.is_err(), "run without session must error");
    let err = result.unwrap_err();
    let stable = err.stable_code().as_str();
    assert_eq!(stable, "LBR-BISECT-001");
}

/// `bisect run` with a script that always returns 128 must surface the
/// non-recoverable exit code through `BisectRunFailed` (LBR-BISECT-002).
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_bisect_run_propagates_fatal_exit_code() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    configure_identity().await;
    let hashes = create_linear_commits(5).await;

    execute_safe(
        Bisect::Start {
            bad: Some(hashes[0].clone()),
            good: Some(hashes[4].clone()),
        },
        &OutputConfig::default(),
    )
    .await
    .unwrap();

    let result = execute_safe(
        Bisect::Run {
            cmd: vec!["sh".to_string(), "-c".to_string(), "exit 128".to_string()],
        },
        &OutputConfig::default(),
    )
    .await;
    assert!(result.is_err(), "exit 128 must abort bisect run");
    let err = result.unwrap_err();
    let stable = err.stable_code().as_str();
    assert_eq!(stable, "LBR-BISECT-002");

    // Clean up so the next test in the suite starts fresh.
    let _ = execute_safe(Bisect::Reset { rev: None }, &OutputConfig::default()).await;
}
