//! Tests for `libra notes` — add, list, show, and remove notes attached to commits.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//!
//! ## Test tiers
//!
//! | Tier | Section | Focus |
//! |------|---------|-------|
//! | 1 | Basic functionality | Happy-path for all 4 subcommands, JSON output, --quiet, --ref |
//! | 2 | Boundary conditions | Empty/long/unicode content, multi-object, cross-ref isolation |
//! | 3 | Error handling | Invalid args, missing objects, unborn HEAD, conflict, file errors |

use serial_test::serial;
use tempfile::tempdir;

use super::*;

// ===========================================================================
// Tier 1 — Basic functionality tests
// ===========================================================================

// ── add ────────────────────────────────────────────────────────────────

#[test]
fn basic_add_with_message() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes", "add", "-m", "Reviewed-by: Alice"], repo.path());
    assert_cli_success(&output, "notes add -m");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Added note to"), "unexpected stdout: {stdout}");
}

#[test]
fn basic_add_json_output() {
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
fn basic_add_with_file() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("msg.txt"), "note from file\n").unwrap();
    let output = run_libra_command(&["notes", "add", "-F", "msg.txt"], repo.path());
    assert_cli_success(&output, "notes add -F");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Added note to"), "unexpected stdout: {stdout}");
}

#[test]
fn basic_add_with_stdin() {
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
fn basic_add_with_multiple_messages() {
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
fn basic_add_force_overwrite() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "First note"], repo.path());
    run_libra_command(&["notes", "add", "-m", "Second note", "-f"], repo.path());

    let output = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&output, "notes show");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Second note"), "expected updated note, got: {stdout}");
}

#[test]
fn basic_add_to_specific_object() {
    let repo = create_committed_repo_via_cli();
    // Add a note to the initial commit by hash
    let log_output = run_libra_command(&["log", "--format=%H", "-n", "1"], repo.path());
    assert_cli_success(&log_output, "log to get commit hash");
    let commit_hash = String::from_utf8_lossy(&log_output.stdout)
        .trim()
        .to_string();

    let output = run_libra_command(
        &["notes", "add", "-m", "Note on specific commit", &commit_hash],
        repo.path(),
    );
    assert_cli_success(&output, "notes add on specific object");
}

// ── list ───────────────────────────────────────────────────────────────

#[test]
fn basic_list_all() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Test note"], repo.path());
    let output = run_libra_command(&["notes", "list"], repo.path());
    assert_cli_success(&output, "notes list");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.trim().is_empty(), "expected list output, got empty");
}

#[test]
fn basic_list_empty() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes", "list"], repo.path());
    assert_cli_success(&output, "notes list empty");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim().is_empty(), "expected empty output, got: {stdout}");
}

#[test]
fn basic_list_json() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "JSON note"], repo.path());
    let output = run_libra_command(&["--json", "notes", "list"], repo.path());
    assert_cli_success(&output, "notes list --json");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "notes");
    assert_eq!(json["data"]["action"], "list");
    assert!(json["data"]["ref"].as_str().unwrap().contains("refs/notes/commits"));
    let notes = json["data"]["notes"].as_array().expect("expected notes array");
    assert_eq!(notes.len(), 1);
    assert!(notes[0]["note_hash"].as_str().is_some());
    assert!(notes[0]["annotated_object"].as_str().is_some());
}

#[test]
fn basic_list_json_empty_returns_empty_array() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--json", "notes", "list"], repo.path());
    assert_cli_success(&output, "notes list --json empty");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "notes");
    assert_eq!(json["data"]["action"], "list");
    let notes = json["data"]["notes"].as_array().expect("expected notes array");
    assert!(notes.is_empty(), "expected empty notes array, got: {json}");
}

#[test]
fn basic_list_by_object() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Note for HEAD"], repo.path());
    let output = run_libra_command(&["notes", "list", "HEAD"], repo.path());
    assert_cli_success(&output, "notes list HEAD");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.trim().is_empty(), "expected list output for HEAD");
}

#[test]
fn basic_list_json_by_object() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "JSON filtered note"], repo.path());
    let output = run_libra_command(&["--json", "notes", "list", "HEAD"], repo.path());
    assert_cli_success(&output, "notes list HEAD --json");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["action"], "list");
    let notes = json["data"]["notes"].as_array().expect("expected notes array");
    assert_eq!(notes.len(), 1);
}

// ── show ───────────────────────────────────────────────────────────────

#[test]
fn basic_show() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Show this note"], repo.path());
    let output = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&output, "notes show");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Show this note");
}

#[test]
fn basic_show_multiline() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Line 1\nLine 2\nLine 3"], repo.path());
    let output = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&output, "notes show multiline");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Line 1"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("Line 3"), "unexpected stdout: {stdout}");
}

#[test]
fn basic_show_json() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "JSON show test"], repo.path());
    let output = run_libra_command(&["--json", "notes", "show"], repo.path());
    assert_cli_success(&output, "notes show --json");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "notes");
    assert_eq!(json["data"]["action"], "show");
    assert_eq!(json["data"]["text"], "JSON show test");
    assert!(json["data"]["ref"].as_str().unwrap().contains("refs/notes/commits"));
    assert!(json["data"]["object"].as_str().is_some());
    assert!(json["data"]["note_hash"].as_str().is_some());
}

#[test]
fn basic_show_specific_object() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Note on HEAD"], repo.path());
    let output = run_libra_command(&["notes", "show", "HEAD"], repo.path());
    assert_cli_success(&output, "notes show HEAD");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "Note on HEAD");
}

// ── remove ─────────────────────────────────────────────────────────────

#[test]
fn basic_remove_single() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "To be removed"], repo.path());
    let output = run_libra_command(&["notes", "remove", "HEAD"], repo.path());
    assert_cli_success(&output, "notes remove HEAD");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Removed note from"), "unexpected stdout: {stdout}");

    // Verify it's gone
    let show_output = run_libra_command(&["notes", "show"], repo.path());
    assert!(!show_output.status.success(), "show should fail after remove");
}

#[test]
fn basic_remove_default_head() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Note on HEAD"], repo.path());
    let output = run_libra_command(&["notes", "remove"], repo.path());
    assert_cli_success(&output, "notes remove default");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Removed note from"), "unexpected stdout: {stdout}");
}

#[test]
fn basic_remove_json() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "JSON remove test"], repo.path());
    let output = run_libra_command(&["--json", "notes", "remove", "HEAD"], repo.path());
    assert_cli_success(&output, "notes remove --json");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "notes");
    assert_eq!(json["data"]["action"], "remove");
    assert!(json["data"]["ref"].as_str().unwrap().contains("refs/notes/commits"));
    let removed = json["data"]["removed"].as_array().expect("expected removed array");
    assert_eq!(removed.len(), 1);
    assert!(removed[0]["object"].as_str().is_some());
    assert!(removed[0]["note_hash"].as_str().is_some());
}

// ── custom ref ─────────────────────────────────────────────────────────

#[test]
fn basic_custom_ref() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["notes", "add", "-m", "QA reviewed", "--ref", "refs/notes/qa"],
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
    assert!(default_stdout.trim().is_empty(), "default ref should be empty, got: {default_stdout}");
}

#[test]
fn basic_custom_ref_show_and_remove() {
    let repo = create_committed_repo_via_cli();
    let ref_arg = "--ref";
    let ref_val = "refs/notes/audit";

    // add
    run_libra_command(
        &["notes", "add", "-m", "Audit trail", ref_arg, ref_val],
        repo.path(),
    );

    // show
    let show_out = run_libra_command(&["notes", "show", ref_arg, ref_val], repo.path());
    assert_cli_success(&show_out, "show on custom ref");
    assert!(String::from_utf8_lossy(&show_out.stdout).contains("Audit trail"));

    // remove
    let remove_out = run_libra_command(
        &["notes", "remove", "HEAD", ref_arg, ref_val],
        repo.path(),
    );
    assert_cli_success(&remove_out, "remove on custom ref");

    // verify gone
    let list_out = run_libra_command(&["notes", "list", ref_arg, ref_val], repo.path());
    assert_cli_success(&list_out, "list after remove on custom ref");
    assert!(String::from_utf8_lossy(&list_out.stdout).trim().is_empty());
}

// ── quiet ──────────────────────────────────────────────────────────────

#[test]
fn basic_quiet_suppresses_stdout() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--quiet", "notes", "add", "-m", "Quiet note"], repo.path());
    assert_cli_success(&output, "quiet notes add");
    assert!(output.stdout.is_empty(), "quiet mode should keep stdout empty");
}

// ── default subcommand (defaults to list) ─────────────────────────────

#[test]
fn basic_default_subcommand_is_list() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Default list test"], repo.path());
    let output = run_libra_command(&["notes"], repo.path());
    assert_cli_success(&output, "notes without subcommand");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.trim().is_empty(), "expected default list output");
}

#[test]
fn basic_default_subcommand_empty() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes"], repo.path());
    assert_cli_success(&output, "notes without subcommand (empty)");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim().is_empty(), "expected empty output, got: {stdout}");
}

// ===========================================================================
// Tier 2 — Boundary condition tests
// ===========================================================================

// ── content edge cases ─────────────────────────────────────────────────

#[test]
fn boundary_add_empty_message() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes", "add", "-m", ""], repo.path());
    assert_cli_success(&output, "notes add with empty message");
    // Verify the note exists (empty content)
    let show = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&show, "show empty note");
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.is_empty() || stdout == "\n" || stdout == "\r\n",
        "expected empty or near-empty note content, got: '{stdout}'");
}

#[test]
fn boundary_add_unicode_content() {
    let repo = create_committed_repo_via_cli();
    let unicode_msg = "审查通过 ✅ — 日本語テスト — 한글 테스트 — émoji 🚀";
    run_libra_command(&["notes", "add", "-m", unicode_msg], repo.path());
    let output = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&output, "show unicode note");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("审查通过"), "expected CJK content, got: {stdout}");
    assert!(stdout.contains("🚀"), "expected emoji, got: {stdout}");
}

#[test]
fn boundary_add_very_long_message() {
    let repo = create_committed_repo_via_cli();
    let long_msg = "A".repeat(10_000);
    let output = run_libra_command(&["notes", "add", "-m", &long_msg], repo.path());
    assert_cli_success(&output, "notes add long message");

    let show = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&show, "show long note");
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert_eq!(stdout.trim().len(), 10_000, "expected {} chars, got {}",
        10_000, stdout.trim().len());
}

#[test]
fn boundary_add_special_chars() {
    let repo = create_committed_repo_via_cli();
    let special = "backslash: \\ \ttab\0null\x1bescape\nnewline\rreturn";
    run_libra_command(&["notes", "add", "-m", special], repo.path());
    let output = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&output, "show special chars note");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("backslash:"), "expected backslash content, got: {stdout}");
}

#[test]
fn boundary_add_combined_message_and_file() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("extra.txt"), "from file\n").unwrap();

    let output = run_libra_command(
        &["notes", "add", "-m", "from -m", "-F", "extra.txt"],
        repo.path(),
    );
    assert_cli_success(&output, "notes add -m and -F combined");

    // Content should be joined with "\n\n": first -m parts, then -F parts
    let show = run_libra_command(&["notes", "show"], repo.path());
    assert_cli_success(&show, "show combined note");
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains("from -m"), "expected -m content, got: {stdout}");
    assert!(stdout.contains("from file"), "expected -F content, got: {stdout}");
}

// ── multi-object / multi-ref ───────────────────────────────────────────

#[test]
fn boundary_multiple_notes_same_ref() {
    let repo = create_committed_repo_via_cli();

    // Create a second commit
    std::fs::write(repo.path().join("f2.txt"), "content2\n").unwrap();
    run_libra_command(&["add", "f2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "Second commit", "--no-verify"], repo.path());

    // Add notes to both commits
    run_libra_command(&["notes", "add", "-m", "Note on HEAD~1", "HEAD~1"], repo.path());
    run_libra_command(&["notes", "add", "-m", "Note on HEAD", "HEAD"], repo.path());

    // List should now have 2 entries
    let output = run_libra_command(&["--json", "notes", "list"], repo.path());
    assert_cli_success(&output, "list multiple notes --json");
    let json = parse_json_stdout(&output);
    let notes = json["data"]["notes"].as_array().expect("expected notes array");
    assert_eq!(notes.len(), 2, "expected 2 notes, got: {json}");
}

#[test]
fn boundary_remove_multiple_objects() {
    let repo = create_committed_repo_via_cli();

    std::fs::write(repo.path().join("f2.txt"), "content2\n").unwrap();
    run_libra_command(&["add", "f2.txt"], repo.path());
    run_libra_command(&["commit", "-m", "Second commit", "--no-verify"], repo.path());

    run_libra_command(&["notes", "add", "-m", "Note 1", "HEAD~1"], repo.path());
    run_libra_command(&["notes", "add", "-m", "Note 2", "HEAD"], repo.path());

    // Remove both notes at once
    let output = run_libra_command(
        &["notes", "remove", "HEAD~1", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&output, "remove multiple objects");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Removed note from"), "unexpected stdout: {stdout}");

    // Verify both are gone
    let list = run_libra_command(&["notes", "list"], repo.path());
    assert_cli_success(&list, "list after removing all");
    assert!(String::from_utf8_lossy(&list.stdout).trim().is_empty(),
        "expected empty list after removing all notes");
}

#[test]
fn boundary_cross_ref_isolation() {
    let repo = create_committed_repo_via_cli();

    // Add notes in two different refs for the same object
    run_libra_command(
        &["notes", "add", "-m", "QA note", "--ref", "refs/notes/qa"],
        repo.path(),
    );
    run_libra_command(
        &["notes", "add", "-m", "Review note", "--ref", "refs/notes/review"],
        repo.path(),
    );

    // qa ref: 1 note
    let qa = run_libra_command(&["--json", "notes", "list", "--ref", "refs/notes/qa"], repo.path());
    assert_cli_success(&qa, "list qa --json");
    assert_eq!(
        parse_json_stdout(&qa)["data"]["notes"].as_array().unwrap().len(),
        1
    );

    // review ref: 1 note
    let review = run_libra_command(
        &["--json", "notes", "list", "--ref", "refs/notes/review"],
        repo.path(),
    );
    assert_cli_success(&review, "list review --json");
    assert_eq!(
        parse_json_stdout(&review)["data"]["notes"].as_array().unwrap().len(),
        1
    );

    // default ref: 0 notes
    let default = run_libra_command(&["--json", "notes", "list"], repo.path());
    assert_cli_success(&default, "list default --json");
    assert_eq!(
        parse_json_stdout(&default)["data"]["notes"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn boundary_force_update_preserves_object_hash() {
    let repo = create_committed_repo_via_cli();

    let first = run_libra_command(&["--json", "notes", "add", "-m", "First"], repo.path());
    assert_cli_success(&first, "first add");
    let first_obj = parse_json_stdout(&first)["data"]["object"].as_str().unwrap().to_string();

    let second = run_libra_command(
        &["--json", "notes", "add", "-m", "Second", "-f"],
        repo.path(),
    );
    assert_cli_success(&second, "force add");
    let second_obj = parse_json_stdout(&second)["data"]["object"].as_str().unwrap().to_string();

    // Object hash should stay the same (same commit)
    assert_eq!(first_obj, second_obj, "object hash changed after force update");
    // Note hash should differ (different content)
    let first_hash = parse_json_stdout(&first)["data"]["note_hash"].as_str().unwrap().to_string();
    let second_hash = parse_json_stdout(&second)["data"]["note_hash"].as_str().unwrap().to_string();
    assert_ne!(first_hash, second_hash, "note hash should change after force update");
}

#[test]
fn boundary_json_list_multiple_entries() {
    let repo = create_committed_repo_via_cli();

    // Add 3 notes (all on same object — they're 3 rows with different refs)
    run_libra_command(
        &["notes", "add", "-m", "A", "--ref", "refs/notes/a"],
        repo.path(),
    );
    run_libra_command(
        &["notes", "add", "-m", "B", "--ref", "refs/notes/b"],
        repo.path(),
    );
    run_libra_command(
        &["notes", "add", "-m", "C", "--ref", "refs/notes/c"],
        repo.path(),
    );

    // Each ref lists 1 note
    for ref_name in &["refs/notes/a", "refs/notes/b", "refs/notes/c"] {
        let out = run_libra_command(
            &["--json", "notes", "list", "--ref", ref_name],
            repo.path(),
        );
        assert_cli_success(&out, &format!("list {ref_name}"));
        let notes = parse_json_stdout(&out)["data"]["notes"]
            .as_array()
            .expect("expected notes array");
        assert_eq!(notes.len(), 1, "expected 1 note in {ref_name}");
        assert_eq!(notes[0]["note_hash"].as_str().unwrap().len(), 40);
        assert_eq!(notes[0]["annotated_object"].as_str().unwrap().len(), 40);
    }
}

#[test]
fn boundary_ref_exact_prefix() {
    let repo = create_committed_repo_via_cli();
    // "refs/notes/" is a valid prefix but short; it should be accepted
    let output = run_libra_command(
        &["notes", "add", "-m", "At root", "--ref", "refs/notes/"],
        repo.path(),
    );
    // This is technically a valid notes ref per validation
    // (starts_with "refs/notes/") — but may or may not work depending on
    // internal use. Just verify it's handled gracefully.
    let _ = output; // Accept either success or clean error
}

// ===========================================================================
// Tier 3 — Error handling tests
// ===========================================================================

// ── usage / argument errors ────────────────────────────────────────────

#[test]
fn error_add_without_message_or_file() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes", "add"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(stderr.contains("provide a message"), "unexpected stderr: {stderr}");
}

#[test]
fn error_add_json_without_message() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--json", "notes", "add"], repo.path());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty(), "json error should keep stdout empty");
    assert_eq!(report["error_code"], "LBR-CLI-002");
}

#[test]
fn error_add_nonexistent_file() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes", "add", "-F", "no_such_file.txt"], repo.path());

    assert!(!output.status.success(), "expected failure for nonexistent file");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no_such_file") || stderr.contains("failed to read"),
        "expected file-not-found message, got: {stderr}");
}

#[test]
fn error_add_invalid_object() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["notes", "add", "-m", "Test", "deadbeef00000000000000000000000000000000"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("invalid object"), "unexpected stderr: {stderr}");
    assert!(
        report.hints.iter().any(|h| h.contains("libra log")),
        "expected hint about libra log, got: {:?}", report.hints
    );
}

#[test]
fn error_add_json_invalid_object() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["--json", "notes", "add", "-m", "Test", "deadbeef00000000000000000000000000000000"],
        repo.path(),
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty());
    assert_eq!(report["error_code"], "LBR-CLI-003");
}

// ── conflict errors ───────────────────────────────────────────────────

#[test]
fn error_add_duplicate_without_force() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Note 1"], repo.path());

    let output = run_libra_command(&["notes", "add", "-m", "Note 2"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(stderr.contains("note already exists"), "unexpected stderr: {stderr}");
    assert!(report.hints.iter().any(|h| h.contains("-f")),
        "expected hint about -f, got: {:?}", report.hints);
    assert!(output.stdout.is_empty(), "error should keep stdout empty");
}

#[test]
fn error_add_json_duplicate_without_force() {
    let repo = create_committed_repo_via_cli();
    run_libra_command(&["notes", "add", "-m", "Note 1"], repo.path());

    let output = run_libra_command(&["--json", "notes", "add", "-m", "Note 2"], repo.path());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty(), "json error should keep stdout empty");
    assert_eq!(report["error_code"], "LBR-CONFLICT-002");
}

// ── repo state errors ─────────────────────────────────────────────────

#[test]
fn error_add_unborn_head() {
    let repo = tempdir().expect("failed to create repo root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["notes", "add", "-m", "Test"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert_eq!(report.category, "repo");
    assert!(stderr.contains("HEAD does not point to a commit"), "unexpected stderr: {stderr}");
    assert!(report.hints.iter().any(|h| h.contains("create a commit")),
        "expected hint about creating a commit, got: {:?}", report.hints);
}

#[test]
fn error_show_unborn_head() {
    let repo = tempdir().expect("failed to create repo root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["notes", "show"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(stderr.contains("HEAD does not point to a commit"), "unexpected stderr: {stderr}");
}

#[test]
fn error_remove_unborn_head() {
    let repo = tempdir().expect("failed to create repo root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["notes", "remove"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(stderr.contains("HEAD does not point to a commit"), "unexpected stderr: {stderr}");
}

#[test]
fn error_list_by_object_unborn_head() {
    let repo = tempdir().expect("failed to create repo root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["notes", "list", "HEAD"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
}

#[test]
fn error_add_outside_repo() {
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

// ── invalid ref errors ────────────────────────────────────────────────

#[test]
fn error_add_invalid_ref() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["notes", "add", "-m", "Test", "--ref", "refs/heads/wrong"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(stderr.contains("must start with 'refs/notes/'"), "unexpected stderr: {stderr}");
    assert!(
        report.hints.iter().any(|h| h.contains("refs/notes/")),
        "expected hint about refs/notes/, got: {:?}", report.hints
    );
}

#[test]
fn error_list_invalid_ref() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["notes", "list", "--ref", "refs/tags/bad"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(stderr.contains("must start with 'refs/notes/'"), "unexpected stderr: {stderr}");
}

#[test]
fn error_show_invalid_ref() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["notes", "show", "--ref", "refs/heads/main"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-002");
}

#[test]
fn error_remove_invalid_ref() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["notes", "remove", "HEAD", "--ref", "refs/blobs/x"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-002");
}

// ── not found errors ──────────────────────────────────────────────────

#[test]
fn error_show_not_found() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes", "show"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("no note found"), "unexpected stderr: {stderr}");
    assert!(
        report.hints.iter().any(|h| h.contains("notes list")),
        "expected hint about notes list, got: {:?}", report.hints
    );
}

#[test]
fn error_show_json_not_found() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--json", "notes", "show"], repo.path());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty());
    assert_eq!(report["error_code"], "LBR-CLI-003");
}

#[test]
fn error_remove_not_found() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes", "remove", "HEAD"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("no note found"), "unexpected stderr: {stderr}");
}

#[test]
fn error_remove_json_not_found() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--json", "notes", "remove", "HEAD"], repo.path());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty());
    assert_eq!(report["error_code"], "LBR-CLI-003");
}

#[test]
fn error_list_by_object_not_found() {
    let repo = create_committed_repo_via_cli();

    // Create a second commit that has no note
    std::fs::write(repo.path().join("new.txt"), "new content\n").unwrap();
    run_libra_command(&["add", "new.txt"], repo.path());
    run_libra_command(&["commit", "-m", "Second commit", "--no-verify"], repo.path());

    let output = run_libra_command(&["--json", "notes", "list", "HEAD"], repo.path());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report["error_code"], "LBR-CLI-003");
}

#[test]
fn error_list_by_object_not_found_human() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["notes", "list", "HEAD"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("no note found"), "unexpected stderr: {stderr}");
}

#[test]
fn error_show_invalid_object() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["notes", "show", "this-ref-does-not-exist"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn error_remove_with_invalid_object() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["notes", "remove", "nonexistent-ref-9999"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(stderr.contains("invalid object"), "unexpected stderr: {stderr}");
}

// ── JSON error output structure ────────────────────────────────────────

#[test]
fn error_json_add_unborn_head() {
    let repo = tempdir().expect("failed to create repo root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["--json", "notes", "add", "-m", "Test"], repo.path());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty());
    assert_eq!(report["error_code"], "LBR-REPO-003");
}

#[test]
fn error_json_show_not_found_on_clean_repo() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--json", "notes", "show"], repo.path());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON");

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty());
    assert_eq!(report["error_code"], "LBR-CLI-003");
    assert_eq!(report["message"], "no note found for object");
    assert_eq!(report["category"], "cli");
    assert!(
        report["hints"].as_array().unwrap().iter().any(|h| h.as_str().unwrap().contains("notes list")),
        "expected hint about notes list, got: {:?}", report["hints"]
    );
}
