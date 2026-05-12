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
    assert_eq!(data["network"]["mode"], "denied");
    assert!(data["network"]["allowlist"].as_array().is_some());
    assert_eq!(data["proxy_backend"], "none");
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
}

#[test]
fn sandbox_status_human_works_without_repo() {
    let temp = tempfile::tempdir().expect("failed to create tempdir");

    let output = run_libra_command(&["sandbox", "status"], temp.path());

    assert_cli_success(&output, "sandbox status should not require a repository");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Sandbox status"), "stdout: {stdout}");
    assert!(stdout.contains("sandbox_type:"), "stdout: {stdout}");
    assert!(stdout.contains("writable_roots:"), "stdout: {stdout}");
}
