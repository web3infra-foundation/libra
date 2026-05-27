//! Tests for `libra notes` — add, list, show, and remove notes attached to commits.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use serial_test::serial;
use tempfile::tempdir;

use super::*;

// ── add ────────────────────────────────────────────────────────────────

#[test]
fn test_notes_add_with_message() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["notes", "add", "-m", "Reviewed-by: Alice"], repo.path());
    assert_cli_success(&output, "notes add -m");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Added note to"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn test_notes_add_json_output() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["--json", "notes", "add", "-m", "Reviewed-by: Alice"],
        repo.path(),
    );
    assert_cli_success(&output, "notes add --json");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "notes");
    assert_eq!(json["data"]["action"], "add");
    assert!(
        json["data"]["ref"].as_str().unwrap().contains("refs/notes/commits"),
        "expected notes ref, got: {json}"
    );
    assert!(json["data"]["object"].as_str().is_some());
    assert!(json["data"]["note_hash"].as_str().is_some());
}

#[test]
fn test_notes_add_with_file() {
    let repo = create_committed_repo_via_cli();

    std::fs::write(repo.path().join("msg.txt"), "note from file\n").unwrap();

    let output = run_libra_command(
        &["notes", "add", "-F", "msg.txt"],
        repo.path(),
    );
    assert_cli_success(&output, "notes add -F");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Added note to"), "unexpected stdout: {stdout}");
}

#[test]
fn test_notes_add_with_stdin() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command_with_stdin(
        &["notes", "add", "-F", "-"],
        repo.path(),
        "note from stdin\n",
    );
    assert_cli_success(&output, "notes add -F -");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Added note to"), "unexpected stdout: {stdout}");
}

#[test]
fn test_notes_add_with_multiple_messages() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["notes", "add", "-m", "Line 1", "-m", "Line 2"],
        repo.path(),
    );
    assert_cli_success(&output, "notes add multiple -m");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Added note to"), "unexpected stdout: {stdout}");
}

#[test]
fn test_notes_add_force_overwrite() {
    let repo = create_committed_repo_via_cli();

    let output1 = run_libra_command(
        &["notes", "add", "-m", "First note"],
        repo.path(),
    );
    assert_cli_success(&output1, "notes add first");

    let output2 = run_libra_command(
        &["notes", "add", "-m", "Second note", "-f"],
        repo.path(),
    );
    assert_cli_success(&output2, "notes add -f");

    // Verify the note was updated
    let output3 = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&output3, "notes show");
    let stdout = String::from_utf8_lossy(&output3.stdout);
    assert!(stdout.contains("Second note"), "expected updated note, got: {stdout}");
}

#[test]
fn test_notes_add_without_message_errors() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["notes", "add"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        stderr.contains("provide a message"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_notes_add_duplicate_errors_without_force() {
    let repo = create_committed_repo_via_cli();

    let output1 = run_libra_command(
        &["notes", "add", "-m", "Note 1"],
        repo.path(),
    );
    assert_cli_success(&output1, "notes add first");

    let output2 = run_libra_command(
        &["notes", "add", "-m", "Note 2"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output2.stderr);

    assert_eq!(output2.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(
        stderr.contains("note already exists"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        report.hints.iter().any(|h| h.contains("-f")),
        "expected hint about -f, got: {:?}",
        report.hints
    );
}

#[test]
fn test_notes_add_outside_repo_errors() {
    let cwd = tempdir().expect("failed to create non-repo directory");

    let output = run_libra_command(&["notes", "add", "-m", "Test"], cwd.path());
    assert!(!output.status.success(), "expected failure outside repo");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a Libra repository")
            || stderr.contains("no libra")
            || stderr.contains("fatal:"),
        "expected recognizable error stderr, got: {stderr}"
    );
}

#[test]
fn test_notes_add_unborn_head_errors() {
    let repo = tempdir().expect("failed to create repo root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["notes", "add", "-m", "Test"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert_eq!(report.category, "repo");
    assert!(
        stderr.contains("HEAD does not point to a commit"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_notes_add_invalid_ref_errors() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["notes", "add", "-m", "Test", "--ref", "refs/heads/wrong"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        stderr.contains("must start with 'refs/notes/'"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_notes_add_json_duplicate_errors() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "Note 1"], repo.path());

    let output = run_libra_command(
        &["--json", "notes", "add", "-m", "Note 2"],
        repo.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty(), "json error should keep stdout empty");
    assert_eq!(report["error_code"], "LBR-CONFLICT-002");
}

// ── list ───────────────────────────────────────────────────────────────

#[test]
fn test_notes_list_all() {
    let repo = create_committed_repo_via_cli();

    // Add a note first
    run_libra_command(&["notes", "add", "-m", "Test note"], repo.path());

    let output = run_libra_command(&["notes", "list"], repo.path());
    assert_cli_success(&output, "notes list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.trim().is_empty(), "expected list output, got empty");
}

#[test]
fn test_notes_list_empty() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["notes", "list"], repo.path());
    assert_cli_success(&output, "notes list empty");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "expected empty output, got: {stdout}"
    );
}

#[test]
fn test_notes_list_json() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "JSON note"], repo.path());

    let output = run_libra_command(&["--json", "notes", "list"], repo.path());
    assert_cli_success(&output, "notes list --json");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "notes");
    assert_eq!(json["data"]["action"], "list");
    assert!(
        json["data"]["ref"].as_str().unwrap().contains("refs/notes/commits"),
        "expected notes ref, got: {json}"
    );
    let notes = json["data"]["notes"].as_array().expect("expected notes array");
    assert_eq!(notes.len(), 1);
    assert!(notes[0]["note_hash"].as_str().is_some());
    assert!(notes[0]["annotated_object"].as_str().is_some());
}

#[test]
fn test_notes_list_by_object() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "Note for HEAD"], repo.path());

    let output = run_libra_command(&["notes", "list", "HEAD"], repo.path());
    assert_cli_success(&output, "notes list HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.trim().is_empty(), "expected list output for HEAD");
}

#[test]
fn test_notes_list_json_by_object_not_found_errors() {
    let repo = create_committed_repo_via_cli();

    // Create a second commit that has no note
    std::fs::write(repo.path().join("new.txt"), "new content\n").unwrap();
    run_libra_command(&["add", "new.txt"], repo.path());
    run_libra_command(&["commit", "-m", "Second commit", "--no-verify"], repo.path());

    let output = run_libra_command(
        &["--json", "notes", "list", "HEAD"],
        repo.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report["error_code"], "LBR-CLI-003");
}

// ── show ───────────────────────────────────────────────────────────────

#[test]
fn test_notes_show() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "Show this note"], repo.path());

    let output = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&output, "notes show");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Show this note");
}

#[test]
fn test_notes_show_multiline() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(
        &["notes", "add", "-m", "Line 1\nLine 2\nLine 3"],
        repo.path(),
    );

    let output = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&output, "notes show multiline");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Line 1"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("Line 3"), "unexpected stdout: {stdout}");
}

#[test]
fn test_notes_show_json() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "JSON show test"], repo.path());

    let output = run_libra_command(&["--json", "notes", "show"], repo.path());
    assert_cli_success(&output, "notes show --json");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "notes");
    assert_eq!(json["data"]["action"], "show");
    assert_eq!(json["data"]["text"], "JSON show test");
    assert!(
        json["data"]["ref"].as_str().unwrap().contains("refs/notes/commits"),
        "expected notes ref, got: {json}"
    );
    assert!(json["data"]["object"].as_str().is_some());
    assert!(json["data"]["note_hash"].as_str().is_some());
}

#[test]
fn test_notes_show_not_found_errors() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["notes", "show"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        stderr.contains("no note found"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        report.hints.iter().any(|h| h.contains("notes list")),
        "expected hint about notes list, got: {:?}",
        report.hints
    );
}

// ── remove ─────────────────────────────────────────────────────────────

#[test]
fn test_notes_remove_single() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "To be removed"], repo.path());

    let output = run_libra_command(&["notes", "remove", "HEAD"], repo.path());
    assert_cli_success(&output, "notes remove HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Removed note from"),
        "unexpected stdout: {stdout}"
    );

    // Verify it's gone
    let show_output = run_libra_command(&["notes", "show"], repo.path());
    assert!(!show_output.status.success(), "show should fail after remove");
}

#[test]
fn test_notes_remove_default_head() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "Note on HEAD"], repo.path());

    // Remove with no arguments defaults to HEAD
    let output = run_libra_command(&["notes", "remove"], repo.path());
    assert_cli_success(&output, "notes remove default");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Removed note from"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn test_notes_remove_json() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "JSON remove test"], repo.path());

    let output = run_libra_command(&["--json", "notes", "remove", "HEAD"], repo.path());
    assert_cli_success(&output, "notes remove --json");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "notes");
    assert_eq!(json["data"]["action"], "remove");
    assert!(
        json["data"]["ref"].as_str().unwrap().contains("refs/notes/commits"),
        "expected notes ref, got: {json}"
    );
    let removed = json["data"]["removed"]
        .as_array()
        .expect("expected removed array");
    assert_eq!(removed.len(), 1);
    assert!(removed[0]["object"].as_str().is_some());
    assert!(removed[0]["note_hash"].as_str().is_some());
}

#[test]
fn test_notes_remove_not_found_errors() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["notes", "remove", "HEAD"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        stderr.contains("no note found"),
        "unexpected stderr: {stderr}"
    );
}

// ── custom ref ─────────────────────────────────────────────────────────

#[test]
fn test_notes_custom_ref() {
    let repo = create_committed_repo_via_cli();

    // Add a note in a custom namespace
    let output = run_libra_command(
        &[
            "notes",
            "add",
            "-m",
            "QA reviewed",
            "--ref",
            "refs/notes/qa",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "notes add --ref refs/notes/qa");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("refs/notes/qa"), "unexpected stdout: {stdout}");

    // List from custom ref
    let list_output = run_libra_command(
        &["notes", "list", "--ref", "refs/notes/qa"],
        repo.path(),
    );
    assert_cli_success(&list_output, "notes list --ref refs/notes/qa");
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(!list_stdout.trim().is_empty(), "expected list output for qa ref");

    // Default ref should be empty (no notes there)
    let default_list = run_libra_command(&["notes", "list"], repo.path());
    assert_cli_success(&default_list, "notes list default ref");
    let default_stdout = String::from_utf8_lossy(&default_list.stdout);
    assert!(
        default_stdout.trim().is_empty(),
        "default ref should be empty, got: {default_stdout}"
    );
}

// ── quiet ──────────────────────────────────────────────────────────────

#[test]
fn test_notes_quiet_suppresses_stdout() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["--quiet", "notes", "add", "-m", "Quiet note"],
        repo.path(),
    );
    assert_cli_success(&output, "quiet notes add");
    assert!(
        output.stdout.is_empty(),
        "quiet mode should keep stdout empty, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// ── default subcommand (defaults to list) ─────────────────────────────

#[test]
fn test_notes_defaults_to_list() {
    let repo = create_committed_repo_via_cli();

    run_libra_command(&["notes", "add", "-m", "Default list test"], repo.path());

    let output = run_libra_command(&["notes"], repo.path());
    assert_cli_success(&output, "notes without subcommand");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.trim().is_empty(), "expected default list output");
}
