use super::*;
use libra::command::remote::{self, RemoteCmds};
use libra::internal::config::Config;

#[tokio::test]
#[serial]
async fn test_remote_add_creates_entry() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "https://example.com/repo.git".into(),
    })
    .await;

    // Verify the remote URL is stored as expected.
    let remote = Config::remote_config("origin").await;
    assert!(remote.is_some(), "remote should exist after add");
    assert_eq!(remote.unwrap().url, "https://example.com/repo.git");
}

#[tokio::test]
#[serial]
async fn test_remote_remove_deletes_entry() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "https://example.com/repo.git".into(),
    })
    .await;

    remote::execute(RemoteCmds::Remove {
        name: "origin".into(),
    })
    .await;

    // Ensure the entry is gone from configuration.
    let remote = Config::remote_config("origin").await;
    assert!(remote.is_none(), "remote should be removed");
}

#[tokio::test]
#[serial]
async fn test_remote_rename_updates_branch_tracking() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "https://example.com/repo.git".into(),
    })
    .await;

    // Mirror Git's tracking layout for the main branch.
    Config::insert("branch", Some("main"), "remote", "origin").await;
    Config::insert("branch", Some("main"), "merge", "refs/heads/main").await;

    remote::execute(RemoteCmds::Rename {
        old: "origin".into(),
        new: "upstream".into(),
    })
    .await;

    assert!(
        Config::remote_config("origin").await.is_none(),
        "old remote entry should be gone"
    );

    // The new remote name should retain the original URL.
    let renamed = Config::remote_config("upstream").await;
    assert!(renamed.is_some(), "new remote entry should exist");
    assert_eq!(
        renamed.unwrap().url,
        "https://example.com/repo.git",
        "URL should be preserved after rename"
    );

    let branch_remote = Config::get("branch", Some("main"), "remote").await;
    assert_eq!(
        branch_remote.as_deref(),
        Some("upstream"),
        "tracking branch should reference the new remote name"
    );
}

#[tokio::test]
#[serial]
async fn test_remote_rename_conflict_returns_error() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "https://example.com/repo.git".into(),
    })
    .await;
    remote::execute(RemoteCmds::Add {
        name: "upstream".into(),
        url: "https://example.com/upstream.git".into(),
    })
    .await;

    // Attempt to rename into the existing target and expect failure.
    let result = Config::rename_remote("origin", "upstream").await;
    assert!(result.is_err(), "rename into existing name should fail");
}
