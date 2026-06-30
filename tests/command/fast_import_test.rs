//! Integration tests for `libra fast-import`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use tempfile::tempdir;

use super::{
    create_committed_repo_via_cli, init_repo_via_cli, run_libra_command,
    run_libra_command_with_stdin,
};

const STREAM: &str = "blob
mark :1
data 6
hello

commit refs/heads/imported
mark :2
committer Tester <t@example.com> 1700000000 +0000
data 8
initial

M 100644 :1 greeting.txt

done
";

#[test]
fn fast_import_creates_a_branch_and_objects() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let result = run_libra_command_with_stdin(&["fast-import", "--quiet"], repo.path(), STREAM);
    assert_eq!(
        result.status.code(),
        Some(0),
        "fast-import failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    // The imported branch resolves and its commit object reads back.
    let rev = run_libra_command(&["rev-parse", "imported"], repo.path());
    assert_eq!(rev.status.code(), Some(0), "imported branch should resolve");
    let oid = String::from_utf8_lossy(&rev.stdout).trim().to_string();

    // cat-file the commit (no HEAD commit needed) — its message + tree are present.
    let commit = run_libra_command(&["cat-file", "-p", &oid], repo.path());
    assert_eq!(
        commit.status.code(),
        Some(0),
        "cat-file the imported commit"
    );
    let text = String::from_utf8_lossy(&commit.stdout);
    assert!(
        text.contains("initial"),
        "commit should carry the imported message: {text}"
    );
    assert!(
        text.contains("committer Tester"),
        "commit should carry the imported committer: {text}"
    );
}

/// Round-trip: a real `fast-export` stream must import cleanly (idempotently)
/// back into the same repository.
#[test]
fn fast_import_accepts_a_fast_export_stream() {
    let repo = create_committed_repo_via_cli();
    let export = run_libra_command(&["fast-export"], repo.path());
    assert_eq!(export.status.code(), Some(0), "fast-export failed");
    let stream = String::from_utf8(export.stdout).expect("export stream is UTF-8 for a text repo");

    let import = run_libra_command_with_stdin(&["fast-import", "--quiet"], repo.path(), &stream);
    assert_eq!(
        import.status.code(),
        Some(0),
        "round-trip import failed: {}",
        String::from_utf8_lossy(&import.stderr)
    );
}

#[test]
fn fast_import_reads_from_an_input_file() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    let path = repo.path().join("stream.fi");
    fs::write(&path, STREAM).unwrap();

    let result = run_libra_command(
        &["fast-import", "--quiet", "--input", path.to_str().unwrap()],
        repo.path(),
    );
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(
        run_libra_command(&["rev-parse", "imported"], repo.path())
            .status
            .code(),
        Some(0)
    );
}

#[test]
fn fast_import_rejects_out_of_repo_ref() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    let bad = "commit not-a-ref
committer T <t@x> 1700000000 +0000
data 2
hi

done
";
    let result = run_libra_command_with_stdin(&["fast-import"], repo.path(), bad);
    assert_eq!(result.status.code(), Some(128));
}

#[test]
fn fast_import_rejects_duplicate_mark() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    let dup = "blob
mark :1
data 1
a

blob
mark :1
data 1
b

done
";
    let result = run_libra_command_with_stdin(&["fast-import"], repo.path(), dup);
    assert_eq!(result.status.code(), Some(128));
}

#[test]
fn fast_import_enforces_the_input_size_limit() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    // Cap the input at 10 bytes, then feed a much larger stream.
    let cfg = run_libra_command(&["config", "fastimport.maxInputSize", "10"], repo.path());
    assert_eq!(cfg.status.code(), Some(0), "config set should succeed");

    let result = run_libra_command_with_stdin(&["fast-import"], repo.path(), STREAM);
    assert_eq!(
        result.status.code(),
        Some(128),
        "should reject an oversized stream"
    );
}

#[test]
fn fast_import_outside_repository_is_an_error() {
    let dir = tempdir().unwrap();
    let result = run_libra_command_with_stdin(&["fast-import"], dir.path(), STREAM);
    assert_eq!(result.status.code(), Some(128));
}
