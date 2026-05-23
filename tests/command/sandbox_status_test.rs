use super::{
    assert_cli_success, parse_json_stdout, run_libra_command, run_libra_command_with_stdin_and_env,
};

#[test]
fn sandbox_status_json_works_without_repo() {
    let temp = tempfile::tempdir().expect("failed to create tempdir");

    let output = run_libra_command(&["--json", "sandbox", "status"], temp.path());

    assert_cli_success(&output, "sandbox status should not require a repository");
    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "sandbox.status");
    let data = &json["data"];
    assert!(data["platform"].as_str().is_some());
    assert!(data["sandbox_type"].as_str().is_some());
    assert_eq!(data["enforcement"], "best_effort");
    assert_eq!(data["effective_enforcement"], "best_effort");
    assert_eq!(data["network"]["mode"], "denied");
    assert!(data["network"]["allowlist"].as_array().is_some());
    assert_eq!(data["proxy_backend"], "noop");
    assert!(data["writable_roots"].as_array().is_some());
    assert!(data["bwrap_available"].is_boolean());
    assert!(data["bwrap_requested"].is_boolean());
    assert!(data["seatbelt_available"].is_boolean());
    assert!(data["helper_path"]["exists"].is_boolean());
    assert!(data["warnings"].as_array().is_some());
}

#[test]
fn sandbox_status_reports_required_enforcement_from_env() {
    let temp = tempfile::tempdir().expect("failed to create tempdir");

    let output = run_libra_command_with_stdin_and_env(
        &["--json", "sandbox", "status"],
        temp.path(),
        "",
        &[("LIBRA_SANDBOX_ENFORCEMENT", "required")],
    );

    assert_cli_success(&output, "sandbox status should accept required enforcement");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["enforcement"], "required");
    assert_eq!(json["data"]["effective_enforcement"], "required");
}

#[test]
fn sandbox_status_human_works_without_repo() {
    let temp = tempfile::tempdir().expect("failed to create tempdir");

    let output = run_libra_command(&["sandbox", "status"], temp.path());

    assert_cli_success(&output, "sandbox status should not require a repository");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Sandbox status"), "stdout: {stdout}");
    assert!(stdout.contains("sandbox_type:"), "stdout: {stdout}");
    assert!(
        stdout.contains("effective_enforcement:"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("writable_roots:"), "stdout: {stdout}");
}

#[cfg(target_os = "linux")]
#[test]
fn sandbox_status_uses_builtin_bwrap_on_linux_when_helper_is_unavailable() {
    let temp = tempfile::tempdir().expect("failed to create tempdir");
    let bwrap_dir = temp.path().join("bin");
    let bwrap_path = bwrap_dir.join("bwrap");

    std::fs::create_dir_all(&bwrap_dir).expect("failed to create fake bwrap dir");
    std::fs::write(&bwrap_path, "#!/bin/sh\necho fake bwrap\n")
        .expect("failed to write fake bwrap");
    let mut permissions = std::fs::metadata(&bwrap_path)
        .expect("failed to stat fake bwrap")
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&bwrap_path, permissions)
        .expect("failed to make fake bwrap executable");

    let original_path = std::env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", bwrap_dir.display(), original_path);
    let output = run_libra_command_with_stdin_and_env(
        &["--json", "sandbox", "status"],
        temp.path(),
        "",
        &[("LIBRA_LINUX_SANDBOX_EXE", ""), ("PATH", &test_path)],
    );

    assert_cli_success(
        &output,
        "sandbox status should select linux-seccomp when built-in bwrap is usable",
    );
    let json = parse_json_stdout(&output);
    let data = &json["data"];
    assert_eq!(data["sandbox_type"], "linux-seccomp");
    assert_eq!(data["bwrap_available"], true);
    let warnings = data["warnings"]
        .as_array()
        .expect("warnings should be present");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .is_some_and(|value| value.contains("using built-in bwrap"))
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn sandbox_status_prefers_bwrap_when_configured_helper_is_not_executable() {
    let temp = tempfile::tempdir().expect("failed to create tempdir");
    let bwrap_dir = temp.path().join("bin");
    let bwrap_path = bwrap_dir.join("bwrap");
    let helper_path = temp.path().join("libra-linux-sandbox");

    std::fs::create_dir_all(&bwrap_dir).expect("failed to create fake bwrap dir");
    std::fs::write(&bwrap_path, "#!/bin/sh\necho fake bwrap\n")
        .expect("failed to write fake bwrap");
    let mut permissions = std::fs::metadata(&bwrap_path)
        .expect("failed to stat fake bwrap")
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    permissions.set_mode(0o755);
    std::fs::set_permissions(&bwrap_path, permissions)
        .expect("failed to make fake bwrap executable");

    std::fs::write(&helper_path, b"not executable file").expect("failed to write fake helper");
    let mut helper_permissions = std::fs::metadata(&helper_path)
        .expect("failed to stat fake helper")
        .permissions();
    helper_permissions.set_mode(0o644);
    std::fs::set_permissions(&helper_path, helper_permissions)
        .expect("failed to make helper non-executable");

    let original_path = std::env::var("PATH").unwrap_or_default();
    let test_path = format!("{}:{}", bwrap_dir.display(), original_path);
    let output = run_libra_command_with_stdin_and_env(
        &["--json", "sandbox", "status"],
        temp.path(),
        "",
        &[
            (
                "LIBRA_LINUX_SANDBOX_EXE",
                helper_path.to_str().expect("helper path should be utf-8"),
            ),
            ("PATH", &test_path),
        ],
    );

    assert_cli_success(
        &output,
        "sandbox status should select linux-seccomp when helper is unavailable but bwrap is usable",
    );
    let json = parse_json_stdout(&output);
    let data = &json["data"];
    assert_eq!(data["sandbox_type"], "linux-seccomp");
    assert_eq!(data["bwrap_available"], true);
    let warnings = data["warnings"]
        .as_array()
        .expect("warnings should be present");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .is_some_and(|value| value.contains("not executable; using built-in bwrap"))
    }));
}

/// `libra sandbox --help` surfaces the EXAMPLES banner so users see the
/// three supported invocations (human / JSON / machine forms of
/// `status`) without having to read the design doc. Cross-cutting
/// `--help` EXAMPLES rollout per `docs/improvement/README.md` item B.
#[test]
fn test_sandbox_help_lists_examples_banner() {
    let temp = tempfile::tempdir().expect("failed to create tempdir");
    let output = run_libra_command(&["sandbox", "--help"], temp.path());
    assert!(
        output.status.success(),
        "sandbox --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "sandbox --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra sandbox status",
        "libra sandbox --json status",
        "libra sandbox --machine status",
    ] {
        assert!(
            stdout.contains(invocation),
            "sandbox --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
