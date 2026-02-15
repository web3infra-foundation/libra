use std::fs;

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

    fs::create_dir_all(&workdir).unwrap();

    let args = InitArgs {
        bare: false,
        template: None,
        initial_branch: None,
        repo_directory: workdir.to_str().unwrap().to_string(),
        quiet: false,
        shared: None,
        object_format: None,
        ref_format: None,
        separate_git_dir: Some(storage.to_str().unwrap().to_string()),
    };

    init(args).await.unwrap();

    let link_path = workdir.join(".libra");
    assert!(
        link_path.is_file(),
        ".libra in working directory should be a file when using --separate-git-dir"
    );

    let content = fs::read_to_string(&link_path).unwrap();
    assert!(
        content.trim_start().starts_with("gitdir:"),
        "link file should start with 'gitdir:'"
    );

    assert!(
        storage.join("objects").is_dir(),
        "objects directory should be created in separate storage dir"
    );
    assert!(
        storage.join("libra.db").is_file(),
        "database file should be created in separate storage dir"
    );
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
        separate_git_dir: Some(storage.to_str().unwrap().to_string()),
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
        "working_dir should be the work tree when using --separate-git-dir"
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
        separate_git_dir: Some(dir.join("storage").to_str().unwrap().to_string()),
    };

    let res: Result<_, _> = init(args).await;
    assert!(
        res.is_err(),
        "init should error when both --bare and --separate-git-dir are specified"
    );
}
