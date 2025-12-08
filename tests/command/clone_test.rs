use libra::command;
use libra::command::clone::CloneArgs;
use libra::internal::head::Head;
use libra::utils::test;
use serial_test::serial;
use tempfile::tempdir;

#[tokio::test]
#[serial]
#[ignore]
/// Test the clone command with a specific branch
async fn test_clone_branch() {
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let remote_url = "https://gitee.com/pikady/mega-libra-clone-branch-test.git".to_string();

    command::clone::execute(CloneArgs {
        remote_repo: remote_url,
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
    })
    .await;

    // Verify that the `.libra` directory exists
    let libra_dir = temp_path.path().join(".libra");
    assert!(libra_dir.exists());

    // Verify the Head reference
    match Head::current().await {
        Head::Branch(current_branch) => {
            assert_eq!(current_branch, "dev");
        }
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
#[ignore]
/// Test the clone command with the default branch
async fn test_clone_default_branch() {
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let remote_url = "https://gitee.com/pikady/mega-libra-clone-branch-test.git".to_string();

    command::clone::execute(CloneArgs {
        remote_repo: remote_url,
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: None,
    })
    .await;

    // Verify that the `.libra` directory exists
    let libra_dir = temp_path.path().join(".libra");
    assert!(libra_dir.exists());

    // Verify the Head reference
    match Head::current().await {
        Head::Branch(current_branch) => {
            assert_eq!(current_branch, "master");
        }
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
#[ignore]
/// Test the clone command with an empty repository
async fn test_clone_empty_repo() {
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let remote_url = "https://gitee.com/pikady/mega-libra-empty-repo.git".to_string();

    command::clone::execute(CloneArgs {
        remote_repo: remote_url,
        local_path: Some(temp_path.path().to_str().unwrap().to_string()),
        branch: None,
    })
    .await;

    // Verify that the `.libra` directory exists
    let libra_dir = temp_path.path().join(".libra");
    assert!(libra_dir.exists());

    // Verify the Head reference
    match Head::current().await {
        Head::Branch(current_branch) => {
            assert_eq!(current_branch, "master");
        }
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
#[ignore]
/// Test the clone command with an existing empty directory
async fn test_clone_to_existing_empty_dir() {
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());
    let repo_path = temp_path.path().join("mega-libra-clone-branch-test");
    std::fs::create_dir(&repo_path).unwrap();

    let remote_url = "https://gitee.com/pikady/mega-libra-clone-branch-test.git".to_string();

    command::clone::execute(CloneArgs {
        remote_repo: remote_url,
        local_path: Some(repo_path.to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
    })
    .await;

    // Verify that the `.libra` directory exists
    let libra_dir = repo_path.join(".libra");
    assert!(libra_dir.exists());

    // Verify the Head reference
    match Head::current().await {
        Head::Branch(current_branch) => {
            assert_eq!(current_branch, "dev");
        }
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
#[ignore]
/// Test when a file exists inside the target directory
async fn test_clone_to_existing_dir() {
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let repo_path = temp_path.path().join("mega-libra-clone-branch-test");
    std::fs::create_dir(&repo_path).unwrap();
    let dummy_file = repo_path.join("exists.txt");
    std::fs::write(&dummy_file, "test").unwrap();

    let remote_url = "https://gitee.com/pikady/mega-libra-clone-branch-test.git".to_string();

    command::clone::execute(CloneArgs {
        remote_repo: remote_url,
        local_path: Some(repo_path.to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
    })
    .await;

    // Make sure that the `.libra` directory not exists
    let libra_dir = repo_path.join(".libra");
    assert!(!libra_dir.exists());
    // Make sure that the pre-existing file should still exist
    assert!(dummy_file.exists(), "pre-existing file should still exist");
    let content = std::fs::read_to_string(&dummy_file).unwrap();
    // Make sure that the pre-existing file content should remain unchanged
    assert_eq!(
        content, "test",
        "pre-existing file content should remain unchanged"
    );
}

#[tokio::test]
#[serial]
#[ignore]
/// Test the clone command in the case where a file with the same name as the target directory already exists.
async fn test_clone_to_dir_with_existing_file_name() {
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let conflict_path = temp_path.path().join("mega-libra-clone-branch-test");
    std::fs::write(&conflict_path, "test").unwrap();

    let remote_url = "https://gitee.com/pikady/mega-libra-clone-branch-test.git".to_string();

    command::clone::execute(CloneArgs {
        remote_repo: remote_url,
        local_path: Some(conflict_path.to_str().unwrap().to_string()),
        branch: Some("dev".to_string()),
    })
    .await;

    // Verify that the `.libra` directory does not exist
    let libra_dir = conflict_path.join(".libra");
    assert!(!libra_dir.exists());
    // Make sure that the pre-existing file should still exist
    assert!(
        conflict_path.exists(),
        "pre-existing file should still exist"
    );
    let content = std::fs::read_to_string(&conflict_path).unwrap();
    // Make sure that the pre-existing file content should remain unchanged
    assert_eq!(
        content, "test",
        "pre-existing file content should remain unchanged"
    );
}
