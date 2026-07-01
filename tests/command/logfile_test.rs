//! Integration tests for `libra logfile`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network, no repo).

use std::process::Output;

use tempfile::tempdir;

use super::{parse_json_stdout, run_libra_command_with_stdin_and_env};

/// Run `libra logfile info` in `cwd` with the given extra env, no repo required.
fn logfile_info(cwd: &std::path::Path, extra_env: &[(&str, &str)]) -> Output {
    run_libra_command_with_stdin_and_env(&["logfile", "info"], cwd, "", extra_env)
}

#[test]
fn logfile_info_reports_disabled_by_default() {
    let dir = tempdir().unwrap();
    let result = logfile_info(dir.path(), &[]);
    assert_eq!(
        result.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("logging: disabled"),
        "no log env should report disabled, got: {stdout}"
    );
}

#[test]
fn logfile_info_reports_file_and_rotation() {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("libra.log");
    let log_path_str = log_path.to_str().unwrap();
    let result = logfile_info(
        dir.path(),
        &[
            ("LIBRA_LOG_FILE", log_path_str),
            ("LIBRA_LOG_ROTATION", "daily"),
        ],
    );
    assert_eq!(result.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("logging: enabled"), "got: {stdout}");
    assert!(stdout.contains("rotation: daily"), "got: {stdout}");
    assert!(stdout.contains(log_path_str), "got: {stdout}");
}

#[test]
fn logfile_info_json_envelope() {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("libra.log");
    let result = run_libra_command_with_stdin_and_env(
        &["--json", "logfile", "info"],
        dir.path(),
        "",
        &[
            ("LIBRA_LOG_FILE", log_path.to_str().unwrap()),
            ("LIBRA_LOG_ROTATION", "hourly"),
        ],
    );
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(json["data"]["enabled"].as_bool(), Some(true));
    assert_eq!(json["data"]["rotation"].as_str(), Some("hourly"));
    assert!(json["data"]["file"].as_str().is_some());
}

/// Under rotation, the size is summed across the actual rolled `<name>.<suffix>`
/// files on disk (not a bare stat of the base path, which would miss the
/// date-suffixed active file).
#[test]
fn logfile_info_sums_rolled_file_sizes() {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("libra.log");
    // Simulate a rolled file on disk (tracing-appender writes `<name>.<date>`).
    std::fs::write(dir.path().join("libra.log.2026-07-01"), b"12345").unwrap();

    let result = run_libra_command_with_stdin_and_env(
        &["--json", "logfile", "info"],
        dir.path(),
        "",
        &[
            ("LIBRA_LOG_FILE", log_path.to_str().unwrap()),
            ("LIBRA_LOG_ROTATION", "daily"),
        ],
    );
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(
        json["data"]["size_bytes"].as_u64(),
        Some(5),
        "should sum the rolled file's 5 bytes"
    );
    assert_eq!(json["data"]["file_count"].as_u64(), Some(1));
}

/// An unknown rotation value falls back to `never` (not an error).
#[test]
fn logfile_info_unknown_rotation_is_never() {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("libra.log");
    let result = logfile_info(
        dir.path(),
        &[
            ("LIBRA_LOG_FILE", log_path.to_str().unwrap()),
            ("LIBRA_LOG_ROTATION", "bogus"),
        ],
    );
    assert_eq!(result.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("rotation: never"), "got: {stdout}");
}
