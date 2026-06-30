//! Integration tests for `libra apply --check`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use tempfile::{TempDir, tempdir};

use super::{parse_json_stdout, run_libra_command, run_libra_command_with_stdin};

fn init_repo() -> TempDir {
    let repo = tempdir().unwrap();
    assert!(run_libra_command(&["init"], repo.path()).status.success());
    repo
}

/// A clean single-file modification: change line 2 of `f.txt`.
const MODIFY_PATCH: &str = "\
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,3 @@
 a
-b
+B
 c
";

#[test]
fn apply_check_clean_modification_exits_0() {
    let repo = init_repo();
    fs::write(repo.path().join("f.txt"), "a\nb\nc\n").unwrap();
    fs::write(repo.path().join("p.diff"), MODIFY_PATCH).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "clean patch should apply: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // --check must not modify the file.
    assert_eq!(
        fs::read_to_string(repo.path().join("f.txt")).unwrap(),
        "a\nb\nc\n"
    );
}

#[test]
fn apply_check_non_matching_context_exits_1() {
    let repo = init_repo();
    fs::write(repo.path().join("f.txt"), "x\ny\nz\n").unwrap();
    fs::write(repo.path().join("p.diff"), MODIFY_PATCH).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(out.status.code(), Some(1), "a non-applying patch exits 1");
}

#[test]
fn apply_check_new_file_exits_0() {
    let repo = init_repo();
    let patch = "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n";
    fs::write(repo.path().join("p.diff"), patch).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "a new-file patch should apply: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apply_check_multi_file_exits_0() {
    let repo = init_repo();
    fs::write(repo.path().join("f.txt"), "a\nb\nc\n").unwrap();
    fs::write(repo.path().join("g.txt"), "1\n2\n3\n").unwrap();
    let patch =
        format!("{MODIFY_PATCH}--- a/g.txt\n+++ b/g.txt\n@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3\n");
    fs::write(repo.path().join("p.diff"), patch).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "a clean multi-file patch should apply: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apply_check_respects_strip_p0() {
    let repo = init_repo();
    fs::write(repo.path().join("f.txt"), "a\nb\nc\n").unwrap();
    // No a/ b/ prefixes; -p0 keeps the path as-is.
    let patch = "--- f.txt\n+++ f.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
    fs::write(repo.path().join("p.diff"), patch).unwrap();
    let out = run_libra_command(&["apply", "--check", "-p0", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "-p0 patch should apply: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apply_check_reads_stdin() {
    let repo = init_repo();
    fs::write(repo.path().join("f.txt"), "a\nb\nc\n").unwrap();
    let out = run_libra_command_with_stdin(&["apply", "--check"], repo.path(), MODIFY_PATCH);
    assert_eq!(
        out.status.code(),
        Some(0),
        "patch from stdin should apply: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apply_security_rejects_path_traversal() {
    let repo = init_repo();
    let patch = "--- a/x\n+++ b/../escape.txt\n@@ -0,0 +1,1 @@\n+x\n";
    fs::write(repo.path().join("p.diff"), patch).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "`..` traversal must be rejected"
    );
}

#[test]
fn apply_security_rejects_libra_internal_path() {
    let repo = init_repo();
    let patch = "--- /dev/null\n+++ b/.libra/evil\n@@ -0,0 +1,1 @@\n+x\n";
    fs::write(repo.path().join("p.diff"), patch).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "writing inside .libra must be rejected"
    );
}

#[test]
fn apply_security_rejects_absolute_path_after_strip() {
    let repo = init_repo();
    // An absolute target must be rejected even though -p1 would turn it relative.
    let patch = "--- a/x\n+++ /abs/evil.txt\n@@ -0,0 +1,1 @@\n+x\n";
    fs::write(repo.path().join("p.diff"), patch).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "an absolute patch path must be rejected before stripping: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apply_check_clean_deletion_exits_0() {
    let repo = init_repo();
    fs::write(repo.path().join("f.txt"), "a\nb\n").unwrap();
    let patch = "--- a/f.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
    fs::write(repo.path().join("p.diff"), patch).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "a clean full-file deletion should apply: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apply_requires_check_flag() {
    let repo = init_repo();
    fs::write(repo.path().join("p.diff"), MODIFY_PATCH).unwrap();
    let out = run_libra_command(&["apply", "p.diff"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "without --check this version errors out"
    );
}

#[test]
fn apply_check_malformed_patch_is_an_error() {
    let repo = init_repo();
    fs::write(repo.path().join("p.diff"), "this is not a patch\n").unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], repo.path());
    assert_eq!(out.status.code(), Some(128), "a malformed patch exits 128");
}

#[test]
fn apply_check_json_reports_result() {
    let repo = init_repo();
    fs::write(repo.path().join("f.txt"), "a\nb\nc\n").unwrap();
    fs::write(repo.path().join("p.diff"), MODIFY_PATCH).unwrap();
    let out = run_libra_command(&["--json", "apply", "--check", "p.diff"], repo.path());
    assert_eq!(out.status.code(), Some(0));
    let json = parse_json_stdout(&out);
    assert_eq!(json["data"]["applies"].as_bool(), Some(true));
    assert_eq!(json["data"]["files"][0].as_str(), Some("f.txt"));
}

#[test]
fn apply_outside_repository_is_an_error() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("p.diff"), MODIFY_PATCH).unwrap();
    let out = run_libra_command(&["apply", "--check", "p.diff"], dir.path());
    assert_eq!(out.status.code(), Some(128));
}
