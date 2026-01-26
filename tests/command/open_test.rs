//! Tests open command integration to ensure it finds remote correctly.
use libra::{
    command::{
        open,
        remote::{self, RemoteCmds},
    },
    utils::test,
};
use serial_test::serial;
use tempfile::tempdir;

#[tokio::test]
#[serial]
async fn test_open_remote_origin() {
    let repo_dir = tempdir().unwrap();
    test::setup_with_new_libra_in(repo_dir.path()).await;
    let _guard = test::ChangeDirGuard::new(repo_dir.path());

    // Add origin remote
    remote::execute(RemoteCmds::Add {
        name: "origin".into(),
        url: "git@github.com:user/repo.git".into(),
    })
    .await;

    // Test explicit remote
    open::open(open::OpenArgs {
        remote: Some("origin".to_string()),
    })
    .await;

    // Test default remote should find origin
    open::open(open::OpenArgs { remote: None }).await;
}
