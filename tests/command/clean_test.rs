//! Tests clean command removing untracked files with minimal flags.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::{fs, io::Write, process::Command};

use libra::utils::path;

use super::*;

#[tokio::test]
#[serial]
/// Tests dry-run mode does not delete files.
async fn test_clean_dry_run_keeps_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut file = fs::File::create("untracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    clean::execute(CleanArgs {
        dry_run: true,
        force: 0,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests force mode deletes untracked files.
async fn test_clean_force_removes_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut file = fs::File::create("untracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(!std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests requiring -f or -n to proceed.
async fn test_clean_requires_flag() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut file = fs::File::create("untracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .output()
        .expect("failed to execute `libra clean`");

    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal: clean requires -f, -n, or -i"));
    assert!(stderr.contains("Error-Code: LBR-CLI-002"));
    assert!(stderr.contains("Hint:"));

    assert!(std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean does not remove tracked files.
async fn test_clean_force_keeps_tracked_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut file = fs::File::create("tracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    add::execute(AddArgs {
        pathspec: vec![String::from("tracked.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(std::path::Path::new("tracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean removes untracked files in subdirectories.
async fn test_clean_force_removes_nested_untracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::create_dir_all("dir/sub").unwrap();
    let mut file = fs::File::create("dir/sub/untracked.txt").unwrap();
    file.write_all(b"content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(!std::path::Path::new("dir/sub/untracked.txt").exists());
    assert!(std::path::Path::new("dir/sub").exists());
}

#[tokio::test]
#[serial]
/// Tests clean respects ignore rules for untracked files.
async fn test_clean_force_respects_ignore_rules() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::write(".libraignore", "ignored.txt\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from(".libraignore")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    fs::write("ignored.txt", "ignored").unwrap();
    fs::write("normal.txt", "normal").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(std::path::Path::new("ignored.txt").exists());
    assert!(!std::path::Path::new("normal.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean removes multiple untracked files but keeps tracked files.
async fn test_clean_force_multiple_untracked_with_tracked() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let mut tracked = fs::File::create("tracked.txt").unwrap();
    tracked.write_all(b"content").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("tracked.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    fs::write("untracked1.txt", "one").unwrap();
    fs::write("untracked2.txt", "two").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(std::path::Path::new("tracked.txt").exists());
    assert!(!std::path::Path::new("untracked1.txt").exists());
    assert!(!std::path::Path::new("untracked2.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean handles missing index by treating all files as untracked.
async fn test_clean_force_with_missing_index() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let index_path = path::index();
    if index_path.exists() {
        fs::remove_file(index_path).unwrap();
    }

    fs::write("untracked.txt", "content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(!std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests clean reports a fatal error for a corrupted index and keeps files.
async fn test_clean_force_with_corrupted_index_returns_fatal_128() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let index_path = path::index();
    fs::write(&index_path, b"corrupted-index-data").unwrap();

    fs::write("untracked.txt", "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .arg("-f")
        .output()
        .expect("failed to execute `libra clean`");

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal: failed to load index"));
    assert!(stderr.contains("Error-Code: LBR-IO-001"));
    assert!(std::path::Path::new("untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests dry-run output format.
async fn test_clean_dry_run_output_format() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;

    let file_path = test_dir.path().join("untracked.txt");
    fs::write(&file_path, "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .arg("-n")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Would remove untracked.txt"));
}

#[tokio::test]
#[serial]
/// Tests that -f and -n together behave like dry-run (no deletion).
async fn test_clean_force_and_dry_run_prefers_dry_run() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;

    fs::write(test_dir.path().join("untracked.txt"), "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .arg("-f")
        .arg("-n")
        .output()
        .expect("failed to execute `libra clean`");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Would remove untracked.txt"));
    assert!(test_dir.path().join("untracked.txt").exists());
}

#[test]
fn test_clean_force_json_reports_deleted_files() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("generated.txt"), "content").unwrap();

    let output = run_libra_command(&["clean", "-f", "--json"], repo.path());
    assert_cli_success(&output, "clean -f --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "clean");
    assert_eq!(json["data"]["dry_run"], false);
    assert_eq!(
        json["data"]["removed"],
        serde_json::json!(["generated.txt"])
    );
    assert!(
        !repo.path().join("generated.txt").exists(),
        "clean -f should remove the reported file"
    );
}

#[tokio::test]
#[serial]
/// Tests clean can handle relatively long file paths.
async fn test_clean_force_with_long_path() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let long_name = "a".repeat(200);
    let long_path = format!("dir/{long_name}.txt");
    fs::create_dir_all("dir").unwrap();
    fs::write(&long_path, "content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(!std::path::Path::new(&long_path).exists());
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// Tests clean does not delete files outside the workdir via symlinked directories.
async fn test_clean_force_does_not_follow_symlink_dirs() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    let outside_dir = tempdir().unwrap();
    let outside_file = outside_dir.path().join("outside.txt");
    fs::write(&outside_file, "content").unwrap();

    symlink(outside_dir.path(), "linked").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(outside_file.exists());
}

#[cfg(unix)]
#[tokio::test]
#[serial]
/// Tests clean reports a fatal error when deletion is denied.
async fn test_clean_force_permission_error_returns_io_exit_code() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::create_dir_all("protected").unwrap();
    fs::write("protected/untracked.txt", "content").unwrap();

    let mut perms = fs::metadata("protected").unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions("protected", perms).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .arg("clean")
        .arg("-f")
        .output()
        .expect("failed to execute `libra clean`");

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal: failed to remove"));
    assert!(stderr.contains("Error-Code: LBR-IO-002"));
    assert!(std::path::Path::new("protected/untracked.txt").exists());

    let mut perms = fs::metadata("protected").unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions("protected", perms).unwrap();
}

#[tokio::test]
#[serial]
async fn test_clean_json_dry_run_lists_candidates() {
    let repo = tempdir().unwrap();
    test::setup_with_new_libra_in(repo.path()).await;

    fs::write(repo.path().join("alpha.txt"), "alpha").unwrap();
    fs::write(repo.path().join("beta.txt"), "beta").unwrap();

    let output = run_libra_command(&["clean", "-n", "--json"], repo.path());
    assert_cli_success(&output, "clean --json dry-run should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "clean");
    assert_eq!(json["data"]["dry_run"], true);

    let removed = json["data"]["removed"]
        .as_array()
        .expect("removed should be an array");
    assert!(removed.iter().any(|path| path == "alpha.txt"));
    assert!(removed.iter().any(|path| path == "beta.txt"));
}

#[tokio::test]
#[serial]
/// Tests -d flag removes untracked directories.
async fn test_clean_d_flag_removes_untracked_dirs() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create an untracked directory with files
    fs::create_dir_all("untracked_dir/sub").unwrap();
    fs::write("untracked_dir/file.txt", "content").unwrap();
    fs::write("untracked_dir/sub/nested.txt", "nested").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: true,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(!std::path::Path::new("untracked_dir").exists());
}

#[tokio::test]
#[serial]
/// Tests -d flag does not remove directories with tracked files.
async fn test_clean_d_flag_keeps_dirs_with_tracked_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create a directory with a tracked file
    fs::create_dir_all("mixed_dir").unwrap();
    fs::write("mixed_dir/tracked.txt", "tracked").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from("mixed_dir/tracked.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    // Add an untracked file in the same directory
    fs::write("mixed_dir/untracked.txt", "untracked").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: true,
        ignored: false,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    // Directory should still exist because it has tracked files
    assert!(std::path::Path::new("mixed_dir").exists());
    assert!(std::path::Path::new("mixed_dir/tracked.txt").exists());
    assert!(!std::path::Path::new("mixed_dir/untracked.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests -x flag removes ignored files.
async fn test_clean_x_flag_removes_ignored_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create .libraignore and ignored files
    fs::write(".libraignore", "ignored.txt\n*.log\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from(".libraignore")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    fs::write("ignored.txt", "ignored").unwrap();
    fs::write("debug.log", "log content").unwrap();
    fs::write("normal.txt", "normal").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: true,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(!std::path::Path::new("ignored.txt").exists());
    assert!(!std::path::Path::new("debug.log").exists());
    assert!(!std::path::Path::new("normal.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests -X flag removes only ignored files.
async fn test_clean_x_flag_removes_only_ignored_files() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create .libraignore and ignored files
    fs::write(".libraignore", "ignored.txt\n*.log\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from(".libraignore")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    fs::write("ignored.txt", "ignored").unwrap();
    fs::write("debug.log", "log content").unwrap();
    fs::write("normal.txt", "normal").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: true,
        exclude: vec![],
    })
    .await;

    assert!(!std::path::Path::new("ignored.txt").exists());
    assert!(!std::path::Path::new("debug.log").exists());
    assert!(std::path::Path::new("normal.txt").exists());
}

#[tokio::test]
#[serial]
/// Tests --exclude flag excludes matching patterns.
async fn test_clean_exclude_flag_excludes_patterns() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::write("important.txt", "important").unwrap();
    fs::write("temp.log", "log").unwrap();
    fs::write("data.csv", "data").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec!["*.txt".to_string()],
    })
    .await;

    assert!(std::path::Path::new("important.txt").exists());
    assert!(!std::path::Path::new("temp.log").exists());
    assert!(!std::path::Path::new("data.csv").exists());
}

#[tokio::test]
#[serial]
/// Tests --exclude with multiple patterns.
async fn test_clean_exclude_multiple_patterns() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::write("file.txt", "txt").unwrap();
    fs::write("file.log", "log").unwrap();
    fs::write("file.csv", "csv").unwrap();
    fs::write("file.dat", "dat").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: false,
        ignored: false,
        only_ignored: false,
        exclude: vec!["*.txt".to_string(), "*.log".to_string()],
    })
    .await;

    assert!(std::path::Path::new("file.txt").exists());
    assert!(std::path::Path::new("file.log").exists());
    assert!(!std::path::Path::new("file.csv").exists());
    assert!(!std::path::Path::new("file.dat").exists());
}

#[tokio::test]
#[serial]
/// Tests -x and -X together returns an error.
async fn test_clean_x_and_x_together_returns_error() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::write("file.txt", "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .args(["clean", "-f", "-x", "-X"])
        .output()
        .expect("failed to execute `libra clean`");

    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot use -x and -X together"));
}

#[tokio::test]
#[serial]
/// Tests -d with dry-run shows directories that would be removed.
async fn test_clean_d_dry_run_shows_directories() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    fs::create_dir_all("untracked_dir").unwrap();
    fs::write("untracked_dir/file.txt", "content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(test_dir.path())
        .args(["clean", "-n", "-d"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Would remove untracked_dir"));
}

#[tokio::test]
#[serial]
/// Tests -d with -x removes ignored directories.
async fn test_clean_dx_removes_ignored_directories() {
    let test_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(test_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(test_dir.path());

    // Create .libraignore with directory pattern
    fs::write(".libraignore", "ignored_dir/\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec![String::from(".libraignore")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;

    fs::create_dir_all("ignored_dir").unwrap();
    fs::write("ignored_dir/file.txt", "content").unwrap();

    clean::execute(CleanArgs {
        dry_run: false,
        force: 1,
        interactive: false,
        directories: true,
        ignored: true,
        only_ignored: false,
        exclude: vec![],
    })
    .await;

    assert!(!std::path::Path::new("ignored_dir").exists());
}

// ── Batch 0: CLI arg interface + clean.requireForce preflight ──

/// `-f` is counted: `-ff` → 2, `-fff` → 3 (enables nested-repo double-force).
#[test]
fn test_clean_force_count_increments() {
    use clap::Parser;
    assert_eq!(CleanArgs::try_parse_from(["clean", "-f"]).unwrap().force, 1);
    assert_eq!(
        CleanArgs::try_parse_from(["clean", "-ff"]).unwrap().force,
        2
    );
    assert_eq!(
        CleanArgs::try_parse_from(["clean", "-fff"]).unwrap().force,
        3
    );
    assert_eq!(
        CleanArgs::try_parse_from(["clean", "-f", "-f"])
            .unwrap()
            .force,
        2
    );
}

/// `-e <pat>` is the short alias for `--exclude`, and is repeatable.
#[test]
fn test_clean_exclude_short_alias_e() {
    use clap::Parser;
    let args = CleanArgs::try_parse_from(["clean", "-e", "*.log", "-e", "*.tmp", "-n"]).unwrap();
    assert_eq!(args.exclude, vec!["*.log".to_string(), "*.tmp".to_string()]);
    assert!(args.dry_run);
    // `-i` / `--interactive` parses.
    assert!(
        CleanArgs::try_parse_from(["clean", "-i"])
            .unwrap()
            .interactive
    );
}

/// `-i` with `--json` is rejected at preflight (LBR-CLI-002 / exit 129).
#[test]
fn test_clean_interactive_json_conflict_rejected() {
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["clean", "-i", "--json"], repo.path());
    assert_eq!(out.status.code(), Some(129), "interactive+json rejected");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .message
            .contains("cannot use --interactive and --json together"),
        "message: {}",
        report.message
    );
}

/// `-i` with `-n` is rejected at preflight (LBR-CLI-002 / exit 129).
#[test]
fn test_clean_interactive_dryrun_conflict_rejected() {
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["clean", "-i", "-n"], repo.path());
    assert_eq!(out.status.code(), Some(129), "interactive+dry-run rejected");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report
            .message
            .contains("cannot use --interactive and --dry-run together"),
        "message: {}",
        report.message
    );
}

/// `clean.requireForce=false` (local) lets a bare `libra clean` proceed and remove.
#[test]
fn test_clean_requireforce_false_allows_no_force() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("untracked.txt"), "scratch\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["config", "clean.requireForce", "false"], p),
        "set clean.requireForce=false",
    );
    // Bare `clean` (no -f/-n/-i) must now proceed and delete the untracked file.
    let out = run_libra_command(&["clean"], p);
    assert_cli_success(&out, "bare clean allowed under requireForce=false");
    assert!(
        !p.join("untracked.txt").exists(),
        "untracked file removed when requireForce=false"
    );
}

/// A global `clean.requireForce=true` (local unset) still blocks a bare `clean`.
#[test]
fn test_clean_requireforce_true_global_blocks() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("untracked.txt"), "scratch\n").unwrap();
    assert_cli_success(
        &run_libra_command(
            &["config", "set", "--global", "clean.requireForce", "true"],
            p,
        ),
        "set global clean.requireForce=true",
    );
    let out = run_libra_command(&["clean"], p);
    assert_eq!(out.status.code(), Some(129), "bare clean blocked");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        p.join("untracked.txt").exists(),
        "untracked file untouched when blocked"
    );
}

/// Bare `libra clean` (default requireForce) is blocked with the updated message.
#[test]
fn test_clean_missing_mode_blocks_without_force() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("untracked.txt"), "scratch\n").unwrap();
    let out = run_libra_command(&["clean"], p);
    assert_eq!(out.status.code(), Some(129));
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("clean requires -f, -n, or -i"),
        "message: {}",
        report.message
    );
    assert!(p.join("untracked.txt").exists(), "nothing removed");
}
