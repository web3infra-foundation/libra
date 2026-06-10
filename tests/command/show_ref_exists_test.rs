use libra::internal::{
    branch::Branch, config::ConfigKv, db::get_db_conn_instance, head::Head, model::reference,
};
use sea_orm::{ActiveModelTrait, Set};

use super::*;

#[test]
fn test_show_ref_exists_remote_tracking_ref_name() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    runtime.block_on(async {
        let head_hash = Head::current_commit()
            .await
            .expect("expected HEAD commit")
            .to_string();
        ConfigKv::set(
            "remote.origin.url",
            "https://example.invalid/repo.git",
            false,
        )
        .await
        .expect("failed to configure remote");
        Branch::update_branch("refs/remotes/origin/main", &head_hash, Some("origin"))
            .await
            .expect("failed to create remote tracking branch");
    });

    let output = run_libra_command(
        &["show-ref", "--exists", "refs/remotes/origin/main"],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn test_show_ref_exists_unresolvable_tag_row() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    runtime.block_on(async {
        let db = get_db_conn_instance().await;
        reference::ActiveModel {
            name: Set(Some("refs/tags/broken-exists".to_string())),
            kind: Set(reference::ConfigKind::Tag),
            commit: Set(Some("not-a-valid-hash".to_string())),
            remote: Set(None),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("failed to insert broken tag ref");
    });

    let output = run_libra_command(
        &["show-ref", "--exists", "refs/tags/broken-exists"],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
