use std::env;

use libra::{
    command::commit::{self, CommitArgs},
    git_protocol::ServiceType,
    internal::{branch::Branch, protocol::local_client::LocalClient},
    utils::test::{self, ChangeDirGuard},
};
use serial_test::serial;
use tempfile::tempdir;

#[tokio::test]
#[serial]
async fn discovery_reference_restores_current_dir_after_error() {
    let caller_dir = tempdir().unwrap();
    let repo_dir = tempdir().unwrap();

    test::setup_with_new_libra_in(repo_dir.path()).await;
    {
        let _guard = ChangeDirGuard::new(repo_dir.path());
        commit::execute(CommitArgs {
            message: Some("initial".into()),
            allow_empty: true,
            disable_pre: true,
            ..Default::default()
        })
        .await;

        Branch::update_branch("broken", "not-a-valid-hash", None)
            .await
            .unwrap();
    }

    let _caller_guard = ChangeDirGuard::new(caller_dir.path());
    let original_dir = env::current_dir().unwrap();

    let client = LocalClient::from_path(repo_dir.path()).unwrap();
    let error = client
        .discovery_reference(ServiceType::UploadPack)
        .await
        .expect_err("expected corrupt branch storage to fail discovery");

    assert!(
        error.to_string().contains("corrupt"),
        "unexpected error: {error}"
    );
    assert_eq!(
        env::current_dir().unwrap(),
        original_dir,
        "local protocol discovery should restore the original working directory",
    );
}
