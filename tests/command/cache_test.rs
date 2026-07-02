//! Integration tests for `libra cache info`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network, no repo).

use tempfile::tempdir;

use super::{parse_json_stdout, run_libra_command_with_stdin_and_env};

/// With no `LIBRA_STORAGE_*` configured, `cache info` reports the local-only
/// defaults (1 MiB threshold, 200 MiB LRU budget) and needs no repository.
#[test]
fn cache_info_reports_local_defaults() {
    let dir = tempdir().unwrap();
    let result =
        run_libra_command_with_stdin_and_env(&["--json", "cache", "info"], dir.path(), "", &[]);
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(json["data"]["storage_type"].as_str(), Some("local"));
    assert_eq!(json["data"]["tiered"].as_bool(), Some(false));
    assert_eq!(json["data"]["threshold_bytes"].as_u64(), Some(1024 * 1024));
    assert_eq!(
        json["data"]["cache_size_bytes"].as_u64(),
        Some(200 * 1024 * 1024)
    );
}

/// A durable tier (`r2`) plus custom threshold/cache-size env vars are reflected
/// in the resolved config.
#[test]
fn cache_info_reflects_tiered_env_overrides() {
    let dir = tempdir().unwrap();
    let result = run_libra_command_with_stdin_and_env(
        &["--json", "cache", "info"],
        dir.path(),
        "",
        &[
            ("LIBRA_STORAGE_TYPE", "r2"),
            ("LIBRA_STORAGE_THRESHOLD", "2048"),
            ("LIBRA_STORAGE_CACHE_SIZE", "536870912"),
        ],
    );
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(json["data"]["storage_type"].as_str(), Some("r2"));
    assert_eq!(json["data"]["tiered"].as_bool(), Some(true));
    assert_eq!(json["data"]["threshold_bytes"].as_u64(), Some(2048));
    assert_eq!(json["data"]["cache_size_bytes"].as_u64(), Some(536_870_912));
}

/// An unparseable numeric tunable falls back to the default (mirroring the
/// storage backend's lenient parse), so `cache info` never fails on a bad value.
#[test]
fn cache_info_falls_back_to_default_on_bad_numeric() {
    let dir = tempdir().unwrap();
    let result = run_libra_command_with_stdin_and_env(
        &["--json", "cache", "info"],
        dir.path(),
        "",
        &[("LIBRA_STORAGE_THRESHOLD", "not-a-number")],
    );
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(json["data"]["threshold_bytes"].as_u64(), Some(1024 * 1024));
}

/// Faithful mirroring: a wrong-case `LIBRA_STORAGE_TYPE=R2` is NOT a durable
/// tier (the backend matches `s3`/`r2` case-sensitively and falls back to local),
/// so `cache info` must report `tiered=false` — never over-report tiering.
#[test]
fn cache_info_does_not_over_report_wrong_case_storage_type() {
    let dir = tempdir().unwrap();
    let result = run_libra_command_with_stdin_and_env(
        &["--json", "cache", "info"],
        dir.path(),
        "",
        &[("LIBRA_STORAGE_TYPE", "R2")],
    );
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(json["data"]["storage_type"].as_str(), Some("R2"));
    assert_eq!(
        json["data"]["tiered"].as_bool(),
        Some(false),
        "wrong-case type must not report tiered (backend rejects it)"
    );
}

/// Faithful mirroring: a whitespace-padded numeric (`" 2048 "`) is unparseable by
/// the backend's raw `.parse()` and falls back to the default — `cache info` must
/// report the same default, not the trimmed value.
#[test]
fn cache_info_mirrors_backend_raw_numeric_parse() {
    let dir = tempdir().unwrap();
    let result = run_libra_command_with_stdin_and_env(
        &["--json", "cache", "info"],
        dir.path(),
        "",
        &[("LIBRA_STORAGE_THRESHOLD", " 2048 ")],
    );
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(
        json["data"]["threshold_bytes"].as_u64(),
        Some(1024 * 1024),
        "whitespace-padded value is unparseable → default, matching the backend"
    );
}

/// Faithful mirroring: `s3`/`r2` with an explicitly-empty `LIBRA_STORAGE_ACCESS_KEY`
/// makes the backend fall back to local before connecting, so `cache info` must
/// report `tiered=false`.
#[test]
fn cache_info_reports_not_tiered_when_access_key_is_empty() {
    let dir = tempdir().unwrap();
    let result = run_libra_command_with_stdin_and_env(
        &["--json", "cache", "info"],
        dir.path(),
        "",
        &[
            ("LIBRA_STORAGE_TYPE", "r2"),
            ("LIBRA_STORAGE_ACCESS_KEY", ""),
        ],
    );
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(json["data"]["storage_type"].as_str(), Some("r2"));
    assert_eq!(
        json["data"]["tiered"].as_bool(),
        Some(false),
        "empty access key must report non-tiered (backend falls back to local)"
    );
}

/// Human output labels the storage tier and the tunables.
#[test]
fn cache_info_human_output_labels_fields() {
    let dir = tempdir().unwrap();
    let result = run_libra_command_with_stdin_and_env(&["cache", "info"], dir.path(), "", &[]);
    assert_eq!(result.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("storage:"), "should label storage type");
    assert!(stdout.contains("threshold:"), "should label threshold");
    assert!(stdout.contains("cache:"), "should label cache budget");
}
