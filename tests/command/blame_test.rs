use super::*;
use libra::command::{
    add::{self, AddArgs},
    blame::{self, BlameArgs},
    commit::{self, CommitArgs},
    get_target_commit,
    init::{self, InitArgs},
};
use std::fs;
use std::io::Write;
use tempfile::tempdir;

async fn setup_repo_with_hash(
    temp: &tempfile::TempDir,
    object_format: &str,
) -> test::ChangeDirGuard {
    test::setup_clean_testing_env_in(temp.path());
    init::init(InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: temp.path().to_str().unwrap().to_string(),
        quiet: true,
        template: None,
        shared: None,
        object_format: Some(object_format.to_string()),
    })
    .await
    .unwrap();
    test::ChangeDirGuard::new(temp.path())
}

async fn prepare_history() -> (ObjectHash, ObjectHash) {
    // first commit
    let mut f = fs::File::create("foo.txt").unwrap();
    writeln!(f, "line1").unwrap();
    writeln!(f, "line2").unwrap();

    add::execute(AddArgs {
        pathspec: vec!["foo.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("init".into()),
        ..Default::default()
    })
    .await;

    let first = get_target_commit("HEAD").await.unwrap();

    // second commit (modify line2)
    let mut f = fs::File::create("foo.txt").unwrap();
    writeln!(f, "line1").unwrap();
    writeln!(f, "line2-modified").unwrap();

    add::execute(AddArgs {
        pathspec: vec!["foo.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("update".into()),
        ..Default::default()
    })
    .await;

    let second = get_target_commit("HEAD").await.unwrap();
    (first, second)
}

#[tokio::test]
#[serial]
async fn blame_runs_with_sha1() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha1").await;
    prepare_history().await;

    // should not panic for SHA-1 repo
    blame::execute(BlameArgs {
        file: "foo.txt".into(),
        commit: "HEAD".into(),
        line_range: None,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn blame_runs_with_sha256() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha256").await;
    prepare_history().await;

    // should not panic for SHA-256 repo
    blame::execute(BlameArgs {
        file: "foo.txt".into(),
        commit: "HEAD".into(),
        line_range: None,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn blame_rejects_sha1_length_on_sha256_repo() {
    let repo = tempdir().unwrap();
    let _guard = setup_repo_with_hash(&repo, "sha256").await;
    prepare_history().await;

    // Passing a 40-hex (SHA-1 length) commit id into a SHA-256 repo should be rejected.
    let res = get_target_commit("4b825dc642cb6eb9a060e54bf8d69288fbee4904").await;
    assert!(
        res.is_err(),
        "expect get_target_commit to reject SHA-1 length hash in SHA-256 repo"
    );
}
