use std::{fs, process::Command};

use libra::utils::util;
use serial_test::serial;
use tempfile::tempdir;

use super::*;

#[tokio::test]
#[serial]
async fn test_init_with_separate_git_dir_creates_link_and_uses_storage() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let storage = temp_root.path().join("storage");

    let args = InitArgs {
        bare: false,
        template: None,
        initial_branch: None,
        repo_directory: workdir.to_str().unwrap().to_string(),
        quiet: false,
        shared: None,
        object_format: None,
        ref_format: None,
        from_git_repository: None,
        separate_libra_dir: Some(storage.to_str().unwrap().to_string()),
    };

    init(args).await.unwrap();

    let link_path = workdir.join(".libra");
    assert!(link_path.is_file());

    let content = fs::read_to_string(&link_path).unwrap();
    assert!(
        content.trim_start().starts_with("gitdir:"),
        "link file should start with 'gitdir:'"
    );

    assert!(storage.join("objects").is_dir());
    assert!(storage.join("libra.db").is_file());
}

#[tokio::test]
#[serial]
async fn test_repository_detection_with_separate_git_dir() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let storage = temp_root.path().join("storage");

    fs::create_dir_all(&workdir).unwrap();

    let args = InitArgs {
        bare: false,
        template: None,
        initial_branch: None,
        repo_directory: workdir.to_str().unwrap().to_string(),
        quiet: true,
        shared: None,
        object_format: None,
        ref_format: None,
        from_git_repository: None,
        separate_libra_dir: Some(storage.to_str().unwrap().to_string()),
    };

    init(args).await.unwrap();

    let _guard = ChangeDirGuard::new(&workdir);
    let storage_path = fs::canonicalize(util::storage_path()).unwrap();
    let expected_storage = fs::canonicalize(&storage).unwrap();
    let working_dir = fs::canonicalize(util::working_dir()).unwrap();
    let expected_workdir = fs::canonicalize(&workdir).unwrap();

    assert_eq!(
        storage_path, expected_storage,
        "storage_path should resolve to separate storage directory"
    );
    assert_eq!(
        working_dir, expected_workdir,
        "working_dir should be the work tree when using --separate-libra-dir"
    );
}

#[tokio::test]
#[serial]
async fn test_init_rejects_bare_with_separate_git_dir() {
    let temp_root = tempdir().unwrap();
    let dir = temp_root.path().join("repo.git");

    fs::create_dir_all(&dir).unwrap();

    let args = InitArgs {
        bare: true,
        template: None,
        initial_branch: None,
        repo_directory: dir.to_str().unwrap().to_string(),
        quiet: true,
        shared: None,
        object_format: None,
        ref_format: None,
        from_git_repository: None,
        separate_libra_dir: Some(dir.join("storage").to_str().unwrap().to_string()),
    };

    let res: Result<_, _> = init(args).await;
    assert!(
        res.is_err(),
        "init should error when both --bare and --separate-libra-dir are specified"
    );
}

#[test]
#[serial]
fn test_init_warns_on_separate_git_dir_alias() {
    let temp_root = tempdir().unwrap();
    let workdir = temp_root.path().join("work");
    let storage = temp_root.path().join("storage");

    fs::create_dir_all(&workdir).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(&workdir)
        .arg("init")
        .arg("--separate-git-dir")
        .arg(storage.to_str().unwrap())
        .output()
        .expect("Failed to execute libra binary");

    assert!(
        output.status.success(),
        "init with --separate-git-dir should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "warning: `--separate-git-dir` is deprecated; use `--separate-libra-dir` instead"
        ),
        "expected deprecation warning in stderr, got: {stderr}"
    );
}
