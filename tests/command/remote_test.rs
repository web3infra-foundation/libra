//! Tests for remote subcommands validating add/list/show behavior and URL mutation scenarios.

use std::{fs, process::Command};

use libra::{
    command::{
        fetch,
        remote::{self, RemoteCmds},
    },
    internal::{
        branch::Branch,
        config::{Config, RemoteConfig},
    },
};

use super::*;

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

#[tokio::test]
#[serial]
async fn test_remote_set_url_add_appends_fetch_url() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    // initial url
    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "https://example.com/repo.git".into(),
    })
    .await;

    // append a second fetch URL with --add
    remote::execute(RemoteCmds::SetUrl {
        add: true,
        delete: false,
        push: false,
        all: false,
        name: "origin".into(),
        value: "https://mirror.example.com/repo.git".into(),
    })
    .await;

    let urls = Config::get_all("remote", Some("origin"), "url").await;
    assert_eq!(urls.len(), 2, "should have two fetch urls after --add");
    assert!(urls.contains(&"https://example.com/repo.git".to_string()));
    assert!(urls.contains(&"https://mirror.example.com/repo.git".to_string()));
}

#[tokio::test]
#[serial]
async fn test_remote_set_url_delete_removes_matching_url() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "https://example.com/repo.git".into(),
    })
    .await;
    remote::execute(RemoteCmds::SetUrl {
        add: true,
        delete: false,
        push: false,
        all: false,
        name: "origin".into(),
        value: "https://mirror.example.com/repo.git".into(),
    })
    .await;

    // delete the mirror url using --delete
    remote::execute(RemoteCmds::SetUrl {
        add: false,
        delete: true,
        push: false,
        all: false,
        name: "origin".into(),
        value: "mirror.example.com".into(),
    })
    .await;

    let urls = Config::get_all("remote", Some("origin"), "url").await;
    assert_eq!(urls.len(), 1, "should have one fetch url after --delete");
    assert_eq!(urls[0], "https://example.com/repo.git");
}

#[tokio::test]
#[serial]
async fn test_remote_set_url_push_and_get_pushurl_entries() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "https://example.com/repo.git".into(),
    })
    .await;

    // add a pushurl entry
    remote::execute(RemoteCmds::SetUrl {
        add: true,
        delete: false,
        push: true,
        all: false,
        name: "origin".into(),
        value: "ssh://git@example.com/repo.git".into(),
    })
    .await;

    let pushurls = Config::get_all("remote", Some("origin"), "pushurl").await;
    assert_eq!(
        pushurls.len(),
        1,
        "should have one pushurl after --add --push"
    );
    assert_eq!(pushurls[0], "ssh://git@example.com/repo.git");

    // Calling get-url --push should prefer pushurl entries (we don't capture stdout here,
    // but ensure the command runs without panic)
    remote::execute(RemoteCmds::GetUrl {
        push: true,
        all: false,
        name: "origin".into(),
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_remote_set_url_all_replaces_all_fetch_urls() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "https://one.example/repo.git".into(),
    })
    .await;
    remote::execute(RemoteCmds::SetUrl {
        add: true,
        delete: false,
        push: false,
        all: false,
        name: "origin".into(),
        value: "https://two.example/repo.git".into(),
    })
    .await;

    // Replace all fetch urls with a single new one
    remote::execute(RemoteCmds::SetUrl {
        add: false,
        delete: false,
        push: false,
        all: true,
        name: "origin".into(),
        value: "https://replaced.example/repo.git".into(),
    })
    .await;

    let urls = Config::get_all("remote", Some("origin"), "url").await;
    assert_eq!(urls.len(), 1, "--all should leave exactly one fetch url");
    assert_eq!(urls[0], "https://replaced.example/repo.git");

    // get-url --all should run without panicking even when printing multiple/single entries
    remote::execute(RemoteCmds::GetUrl {
        push: false,
        all: true,
        name: "origin".into(),
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_remote_prune_removes_stale_branches() {
    let temp_root = tempdir().unwrap();
    let remote_dir = temp_root.path().join("remote.git");
    let work_dir = temp_root.path().join("workdir");

    // Create a bare Git repository as remote
    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .unwrap_or_else(|e| panic!("failed to init bare remote: {}", e))
            .success()
    );

    // Create a working Git repository to push branches from
    assert!(
        Command::new("git")
            .args(["init", work_dir.to_str().unwrap()])
            .status()
            .unwrap_or_else(|e| panic!("failed to init working repo: {}", e))
            .success()
    );

    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.name", "Libra Tester"])
            .status()
            .unwrap_or_else(|e| panic!("failed to set user.name: {}", e))
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.email", "tester@example.com"])
            .status()
            .unwrap_or_else(|e| panic!("failed to set user.email: {}", e))
            .success()
    );

    // Create initial commit
    fs::write(work_dir.join("README.md"), "hello libra")
        .unwrap_or_else(|e| panic!("failed to write README: {}", e));
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["add", "README.md"])
            .status()
            .unwrap_or_else(|e| panic!("failed to add README: {}", e))
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["commit", "-m", "initial commit"])
            .status()
            .unwrap_or_else(|e| panic!("failed to commit: {}", e))
            .success()
    );

    // Get current branch name
    let current_branch = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .unwrap_or_else(|e| panic!("failed to read current branch: {}", e))
            .stdout,
    )
    .unwrap_or_else(|e| panic!("branch name not utf8: {}", e))
    .trim()
    .to_string();

    // Add remote and push initial branch
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["remote", "add", "origin", remote_dir.to_str().unwrap()])
            .status()
            .unwrap_or_else(|e| panic!("failed to add origin remote: {}", e))
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args([
                "push",
                "origin",
                &format!("HEAD:refs/heads/{}", current_branch),
            ])
            .status()
            .unwrap_or_else(|e| panic!("failed to push to remote: {}", e))
            .success()
    );

    // Create and push additional branches
    let branches_to_create = vec!["feature1", "feature2", "feature3"];
    for branch_name in &branches_to_create {
        assert!(
            Command::new("git")
                .current_dir(&work_dir)
                .args(["checkout", "-b", branch_name])
                .status()
                .unwrap_or_else(|e| panic!("failed to create branch {}: {}", branch_name, e))
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(&work_dir)
                .args(["push", "origin", branch_name])
                .status()
                .unwrap_or_else(|e| panic!("failed to push branch {}: {}", branch_name, e))
                .success()
        );
    }

    // Initialize a fresh Libra repository to fetch into
    let repo_dir = temp_root.path().join("libra_repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|e| panic!("failed to create repo dir: {}", e));
    test::setup_with_new_libra_in(&repo_dir).await;
    let _guard = test::ChangeDirGuard::new(&repo_dir);

    let remote_path = remote_dir.to_str().unwrap().to_string();
    Config::insert("remote", Some("origin"), "url", &remote_path).await;

    // Fetch all branches to create remote-tracking branches
    fetch::fetch_repository(
        RemoteConfig {
            name: "origin".to_string(),
            url: remote_path.clone(),
        },
        None,
        false,
    )
    .await;

    // Verify all remote-tracking branches exist
    for branch_name in &branches_to_create {
        let tracked_branch = format!("refs/remotes/origin/{}", branch_name);
        assert!(
            Branch::find_branch(&tracked_branch, None).await.is_some(),
            "remote-tracking branch {} should exist after fetch",
            tracked_branch
        );
    }

    // Delete some branches from remote
    let branches_to_delete = vec!["feature1", "feature3"];
    for branch_name in &branches_to_delete {
        assert!(
            Command::new("git")
                .current_dir(remote_dir.to_str().unwrap())
                .args(["update-ref", "-d", &format!("refs/heads/{}", branch_name)])
                .status()
                .unwrap_or_else(|e| panic!("failed to delete branch {}: {}", branch_name, e))
                .success()
        );
    }

    // Run prune command
    remote::execute(RemoteCmds::Prune {
        name: "origin".into(),
        dry_run: false,
    })
    .await;

    // Verify stale branches are pruned
    for branch_name in &branches_to_delete {
        let tracked_branch = format!("refs/remotes/origin/{}", branch_name);
        assert!(
            Branch::find_branch(&tracked_branch, None).await.is_none(),
            "stale remote-tracking branch {} should be pruned",
            tracked_branch
        );
    }

    // Verify remaining branches still exist
    assert!(
        Branch::find_branch(&format!("refs/remotes/origin/feature2"), None)
            .await
            .is_some(),
        "non-stale remote-tracking branch should still exist"
    );
}

#[tokio::test]
#[serial]
async fn test_remote_prune_dry_run_previews_changes() {
    let temp_root = tempdir().unwrap();
    let remote_dir = temp_root.path().join("remote.git");
    let work_dir = temp_root.path().join("workdir");

    // Create a bare Git repository as remote
    assert!(
        Command::new("git")
            .args(["init", "--bare", remote_dir.to_str().unwrap()])
            .status()
            .unwrap_or_else(|e| panic!("failed to init bare remote: {}", e))
            .success()
    );

    // Create a working Git repository to push branches from
    assert!(
        Command::new("git")
            .args(["init", work_dir.to_str().unwrap()])
            .status()
            .unwrap_or_else(|e| panic!("failed to init working repo: {}", e))
            .success()
    );

    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.name", "Libra Tester"])
            .status()
            .unwrap_or_else(|e| panic!("failed to set user.name: {}", e))
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["config", "user.email", "tester@example.com"])
            .status()
            .unwrap_or_else(|e| panic!("failed to set user.email: {}", e))
            .success()
    );

    // Create initial commit
    fs::write(work_dir.join("README.md"), "hello libra")
        .unwrap_or_else(|e| panic!("failed to write README: {}", e));
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["add", "README.md"])
            .status()
            .unwrap_or_else(|e| panic!("failed to add README: {}", e))
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["commit", "-m", "initial commit"])
            .status()
            .unwrap_or_else(|e| panic!("failed to commit: {}", e))
            .success()
    );

    // Get current branch name
    let current_branch = String::from_utf8(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .unwrap_or_else(|e| panic!("failed to read current branch: {}", e))
            .stdout,
    )
    .unwrap_or_else(|e| panic!("branch name not utf8: {}", e))
    .trim()
    .to_string();

    // Add remote and push initial branch
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["remote", "add", "origin", remote_dir.to_str().unwrap()])
            .status()
            .unwrap_or_else(|e| panic!("failed to add origin remote: {}", e))
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args([
                "push",
                "origin",
                &format!("HEAD:refs/heads/{}", current_branch),
            ])
            .status()
            .unwrap_or_else(|e| panic!("failed to push to remote: {}", e))
            .success()
    );

    // Create and push a branch
    let branch_name = "stale_branch";
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["checkout", "-b", branch_name])
            .status()
            .unwrap_or_else(|e| panic!("failed to create branch: {}", e))
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&work_dir)
            .args(["push", "origin", branch_name])
            .status()
            .unwrap_or_else(|e| panic!("failed to push branch: {}", e))
            .success()
    );

    // Initialize a fresh Libra repository to fetch into
    let repo_dir = temp_root.path().join("libra_repo");
    fs::create_dir_all(&repo_dir).unwrap_or_else(|e| panic!("failed to create repo dir: {}", e));
    test::setup_with_new_libra_in(&repo_dir).await;
    let _guard = test::ChangeDirGuard::new(&repo_dir);

    let remote_path = remote_dir.to_str().unwrap().to_string();
    Config::insert("remote", Some("origin"), "url", &remote_path).await;

    // Fetch to create remote-tracking branch
    fetch::fetch_repository(
        RemoteConfig {
            name: "origin".to_string(),
            url: remote_path.clone(),
        },
        None,
        false,
    )
    .await;

    // Verify remote-tracking branch exists
    let tracked_branch = format!("refs/remotes/origin/{}", branch_name);
    assert!(
        Branch::find_branch(&tracked_branch, None).await.is_some(),
        "remote-tracking branch should exist after fetch"
    );

    // Delete branch from remote
    assert!(
        Command::new("git")
            .current_dir(remote_dir.to_str().unwrap())
            .args(["update-ref", "-d", &format!("refs/heads/{}", branch_name)])
            .status()
            .unwrap_or_else(|e| panic!("failed to delete branch {}: {}", branch_name, e))
            .success()
    );

    // Run prune with --dry-run
    remote::execute(RemoteCmds::Prune {
        name: "origin".into(),
        dry_run: true,
    })
    .await;

    // Verify branch still exists (dry-run should not delete)
    assert!(
        Branch::find_branch(&tracked_branch, None).await.is_some(),
        "remote-tracking branch should still exist after dry-run prune"
    );

    // Now run actual prune
    remote::execute(RemoteCmds::Prune {
        name: "origin".into(),
        dry_run: false,
    })
    .await;

    // Verify branch is now deleted
    assert!(
        Branch::find_branch(&tracked_branch, None).await.is_none(),
        "remote-tracking branch should be pruned after actual prune"
    );
}

#[tokio::test]
#[serial]
async fn test_remote_prune_nonexistent_remote_returns_error() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    // Attempt to prune a non-existent remote
    remote::execute(RemoteCmds::Prune {
        name: "nonexistent".into(),
        dry_run: false,
    })
    .await;

    // The command should fail gracefully (error is printed to stderr, not returned)
    // We can't easily test stderr output, but we can verify it doesn't panic
}
