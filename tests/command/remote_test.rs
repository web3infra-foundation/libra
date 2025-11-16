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
    assert_eq!(pushurls.len(), 1, "should have one pushurl after --add --push");
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
