//! Tests for `verify-pack`, covering generated v1/v2 indexes, verbose output,
//! JSON output, and corrupt/missing pack failures.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serial_test::serial;

use super::{
    assert_cli_success, init_repo_via_cli, parse_cli_error_stderr, parse_json_stdout,
    run_libra_command,
};

fn packs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/packs")
}

fn copy_pack_to_temp(prefix: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let pack_src = fs::read_dir(packs_dir())
        .expect("read packs dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".pack"))
        })
        .unwrap_or_else(|| panic!("pack fixture with prefix {prefix:?} not found"));
    let pack_dst = dir.path().join(
        pack_src
            .file_name()
            .expect("pack fixture should have file name"),
    );
    fs::copy(&pack_src, &pack_dst).expect("copy pack fixture");
    (dir, pack_dst)
}

fn build_index(repo: &Path, pack_path: &Path, version: &str) -> PathBuf {
    let output = run_libra_command(
        &[
            "index-pack",
            pack_path.to_str().expect("pack path should be UTF-8"),
            "--index-version",
            version,
        ],
        repo,
    );
    assert_cli_success(&output, "index-pack should build fixture index");
    pack_path.with_extension("idx")
}

#[test]
#[serial]
fn verify_pack_accepts_generated_v1_index() {
    let repo = tempfile::tempdir().expect("create repo");
    init_repo_via_cli(repo.path());
    let (_pack_dir, pack_path) = copy_pack_to_temp("small-sha1");
    let idx_path = build_index(repo.path(), &pack_path, "1");

    let output = run_libra_command(
        &["verify-pack", idx_path.to_str().expect("idx path UTF-8")],
        repo.path(),
    );
    assert_cli_success(&output, "verify-pack should accept generated v1 index");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(": ok"),
        "human output should confirm success: {stdout}"
    );
    assert!(
        output.stderr.is_empty(),
        "stderr should be clean: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn verify_pack_accepts_absolute_index_path_outside_repository() {
    let repo = tempfile::tempdir().expect("create repo");
    init_repo_via_cli(repo.path());
    let (_pack_dir, pack_path) = copy_pack_to_temp("small-sha1");
    let idx_path = build_index(repo.path(), &pack_path, "1");
    let outside = tempfile::tempdir().expect("create non-repo cwd");

    let output = run_libra_command(
        &["verify-pack", idx_path.to_str().expect("idx path UTF-8")],
        outside.path(),
    );
    assert_cli_success(
        &output,
        "verify-pack should accept absolute index paths outside a repo",
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(": ok"),
        "outside-repo verification should confirm success: {stdout}"
    );
}

#[test]
#[serial]
fn verify_pack_accepts_generated_v2_index_with_verbose_objects() {
    let repo = tempfile::tempdir().expect("create repo");
    init_repo_via_cli(repo.path());
    let (_pack_dir, pack_path) = copy_pack_to_temp("small-sha1");
    let idx_path = build_index(repo.path(), &pack_path, "2");

    let output = run_libra_command(
        &[
            "verify-pack",
            "-v",
            idx_path.to_str().expect("idx path UTF-8"),
        ],
        repo.path(),
    );
    assert_cli_success(&output, "verify-pack should accept generated v2 index");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line.contains("0x")),
        "verbose output should include crc32 values: {stdout}"
    );
    assert!(
        stdout.trim_end().ends_with(": ok"),
        "verbose output should end with success line: {stdout}"
    );
}

#[test]
#[serial]
fn verify_pack_json_reports_counts_and_hashes() {
    let repo = tempfile::tempdir().expect("create repo");
    init_repo_via_cli(repo.path());
    let (_pack_dir, pack_path) = copy_pack_to_temp("small-sha1");
    let idx_path = build_index(repo.path(), &pack_path, "1");

    let output = run_libra_command(
        &[
            "verify-pack",
            idx_path.to_str().expect("idx path UTF-8"),
            "--json",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "verify-pack --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "verify-pack");
    assert_eq!(json["data"]["verified"], true);
    assert_eq!(json["data"]["index_version"], 1);
    assert!(
        json["data"]["object_count"].as_u64().unwrap_or_default() > 0,
        "object_count should be positive: {json}"
    );
    assert!(
        json["data"]["pack_hash"].as_str().unwrap_or_default().len() >= 40,
        "pack_hash should be present: {json}"
    );
    assert!(
        json["data"].get("objects").is_none(),
        "non-verbose JSON should not include per-object payloads: {json}"
    );
}

#[test]
#[serial]
fn verify_pack_rejects_corrupt_index_entry() {
    let repo = tempfile::tempdir().expect("create repo");
    init_repo_via_cli(repo.path());
    let (_pack_dir, pack_path) = copy_pack_to_temp("small-sha1");
    let idx_path = build_index(repo.path(), &pack_path, "1");

    let mut bytes = fs::read(&idx_path).expect("read generated idx");
    let first_hash_byte = 256 * 4 + 4;
    bytes[first_hash_byte] ^= 0x80;
    fs::write(&idx_path, bytes).expect("write corrupt idx");

    let output = run_libra_command(
        &["verify-pack", idx_path.to_str().expect("idx path UTF-8")],
        repo.path(),
    );
    assert!(
        !output.status.success(),
        "corrupt index should fail verification"
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        human.contains("invalid pack index") || human.contains("pack verification failed"),
        "error should explain verification failure: {human}"
    );
}

#[test]
#[serial]
fn verify_pack_reports_missing_pack_as_read_error() {
    let repo = tempfile::tempdir().expect("create repo");
    init_repo_via_cli(repo.path());
    let (_pack_dir, pack_path) = copy_pack_to_temp("small-sha1");
    let idx_path = build_index(repo.path(), &pack_path, "1");
    fs::remove_file(&pack_path).expect("remove pack fixture");

    let output = run_libra_command(
        &["verify-pack", idx_path.to_str().expect("idx path UTF-8")],
        repo.path(),
    );
    assert!(!output.status.success(), "missing pack should fail");

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-IO-001");
    assert!(
        human.contains("could not open pack file"),
        "error should identify missing pack: {human}"
    );
}
