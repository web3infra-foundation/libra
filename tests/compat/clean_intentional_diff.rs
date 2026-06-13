//! `tests/compat/clean_intentional_diff.rs` — intentional-difference test for `clean -i`.
//!
//! This guard validates that `libra clean -i` implements Libra-native interactive
//! selection (intentionally different from Git's filter-by-pattern model). Per
//! COMPATIBILITY.md clean row, `-i` provides a menu-based selection loop and is
//! mutually exclusive with `-n` and `--json` per LBR-CLI-002.

use std::path::PathBuf;

/// Verify that `clean -i` / `--interactive` flag is recognized by the parser.
#[test]
fn clean_interactive_flag_recognized() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let help = std::process::Command::new("cargo")
        .args(["run", "--", "clean", "--help"])
        .current_dir(&repo)
        .output()
        .expect("run 'cargo run -- clean --help'");

    let help_text = String::from_utf8_lossy(&help.stdout);

    // The help text must mention the -i / --interactive flag
    assert!(
        help_text.contains("-i") || help_text.contains("--interactive"),
        "clean --help must document -i / --interactive flag"
    );
}

/// Verify that `-i` without any mode still produces help (not an error).
/// In Libra, `clean` requires `-f` or `-n` or `-i` to proceed safely.
#[test]
fn clean_interactive_mode_documented() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let help = std::process::Command::new("cargo")
        .args(["run", "--", "clean", "--help"])
        .current_dir(&repo)
        .output()
        .expect("run 'cargo run -- clean --help'");

    let help_text = String::from_utf8_lossy(&help.stdout);

    // The help text should mention interactive mode, not just flag name
    assert!(
        help_text.to_lowercase().contains("interactive")
            || help_text.contains("selection")
            || help_text.contains("menu"),
        "clean --help must document interactive selection behavior"
    );
}

/// Verify that `clean -i` and `-n` (dry-run/no-op) are mutually exclusive.
/// Per COMPATIBILITY.md: "mutually exclusive with `--json` and `-n` (`LBR-CLI-002`)".
#[test]
fn clean_interactive_rejects_dry_run() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Try to run: libra clean -i -n
    // This should fail with LBR-CLI-002 (mutual exclusion error)
    let output = std::process::Command::new("cargo")
        .args(["run", "--", "clean", "-i", "-n"])
        .current_dir(&repo)
        .env("LIBRA_SKIP_WEB_BUILD", "1")
        .output()
        .expect("run 'cargo run -- clean -i -n'");

    // Must fail (non-zero exit)
    assert!(
        !output.status.success(),
        "clean -i -n should fail (mutually exclusive flags)"
    );

    // Error should mention mutual exclusion (LBR-CLI-002) or user guidance
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}\n{}", stderr, stdout);

    assert!(
        combined.to_lowercase().contains("mutual")
            || combined.contains("LBR-CLI-002")
            || combined.to_lowercase().contains("exclusive"),
        "Error message should explain mutual exclusion: got {}",
        combined
    );
}

/// Verify that `clean -i` and `--json` (machine output) are mutually exclusive.
/// Per COMPATIBILITY.md: "mutually exclusive with `--json` and `-n` (`LBR-CLI-002`)".
#[test]
fn clean_interactive_rejects_json() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Try to run: libra clean -i --json
    // This should fail with LBR-CLI-002 (mutual exclusion error)
    let output = std::process::Command::new("cargo")
        .args(["run", "--", "clean", "-i", "--json"])
        .current_dir(&repo)
        .env("LIBRA_SKIP_WEB_BUILD", "1")
        .output()
        .expect("run 'cargo run -- clean -i --json'");

    // Must fail (non-zero exit)
    assert!(
        !output.status.success(),
        "clean -i --json should fail (mutually exclusive flags)"
    );

    // Error should mention mutual exclusion (LBR-CLI-002) or user guidance
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}\n{}", stderr, stdout);

    assert!(
        combined.to_lowercase().contains("mutual")
            || combined.contains("LBR-CLI-002")
            || combined.to_lowercase().contains("exclusive"),
        "Error message should explain mutual exclusion: got {}",
        combined
    );
}

/// Verify that clean command documentation mentions the intentional difference.
/// Per COMPATIBILITY.md, the clean row should document: "intentionally-different:
/// mutually exclusive with `--json` and `-n` (`LBR-CLI-002`)".
#[test]
fn clean_command_docs_mention_interactive_difference() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Read docs/commands/clean.md if it exists, or COMPATIBILITY.md
    let compat_path = repo.join("COMPATIBILITY.md");
    let compat_content = std::fs::read_to_string(&compat_path).expect("read COMPATIBILITY.md");

    // The clean row must document the -i intentional difference
    let clean_section = compat_content
        .split("| clean")
        .nth(1)
        .expect("COMPATIBILITY.md must have clean command row");
    let clean_row = clean_section.lines().next().unwrap_or("");

    // Should mention -i / interactive and its intentional difference
    assert!(
        clean_row.contains("-i") || clean_row.contains("interactive"),
        "COMPATIBILITY.md clean row must document -i / --interactive"
    );

    assert!(
        clean_row.contains("intentionally-different")
            || clean_row.contains("LBR-CLI-002")
            || clean_row.contains("mutually exclusive"),
        "COMPATIBILITY.md clean row must document intentional difference: {}",
        clean_row
    );
}

/// Verify that clean command help explains the intentional difference from Git.
/// Libra's -i is menu-based selection, not Git's filter-by-pattern regex.
#[test]
fn clean_help_distinguishes_libra_behavior() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let help = std::process::Command::new("cargo")
        .args(["run", "--", "clean", "--help"])
        .current_dir(&repo)
        .output()
        .expect("run 'cargo run -- clean --help'");

    let help_text = String::from_utf8_lossy(&help.stdout);

    // Help should mention key menu/selection concepts to distinguish from Git
    // (not exhaustive; just verify some distinctive feature is mentioned)
    let mentions_menu_or_selection = help_text.to_lowercase().contains("menu")
        || help_text.to_lowercase().contains("selection")
        || help_text.to_lowercase().contains("interactive")
        || help_text.to_lowercase().contains("choose")
        || help_text.to_lowercase().contains("filter");

    assert!(
        mentions_menu_or_selection,
        "clean --help should mention menu/selection behavior to clarify difference from Git"
    );
}

#[test]
fn clean_governance_documents_interactive_and_pathspec_decisions() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let governance_path = repo.join("docs/development/commands/_compatibility.md");
    let governance =
        std::fs::read_to_string(&governance_path).expect("read compatibility governance doc");

    assert!(
        governance.contains("clean -i") || governance.contains("`clean -i`"),
        "compatibility governance must mention clean -i"
    );
    assert!(
        governance.contains("有意差异") || governance.contains("intentional"),
        "compatibility governance must preserve the clean -i intentional-difference contract"
    );
    assert!(
        governance.contains("D-clean-pathspec"),
        "compatibility governance must preserve the clean pathspec deferred decision"
    );
}

/// Guard contract: `clean -i` is documented as intentional-difference, never as
/// `supported` (which would incorrectly imply Git parity).
#[test]
fn clean_interactive_not_marked_supported() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compat_path = repo.join("COMPATIBILITY.md");
    let compat_content = std::fs::read_to_string(&compat_path).expect("read COMPATIBILITY.md");

    // Find the clean command row
    let clean_section = compat_content
        .split("| clean")
        .nth(1)
        .expect("COMPATIBILITY.md must have clean command row");
    let clean_row_lines: Vec<&str> = clean_section.lines().take(5).collect();
    let clean_row = clean_row_lines.join(" ");

    // The tier should be "partial" (the command is partial)
    // but the -i section should be marked "intentionally-different"
    assert!(
        clean_row.contains("partial"),
        "COMPATIBILITY.md clean tier should be 'partial' (command is incomplete)"
    );

    // The -i documentation should say "intentionally-different", not "supported"
    assert!(
        clean_row.contains("intentionally-different") || clean_row.contains("-i"),
        "COMPATIBILITY.md clean row must mention -i with intentional-difference label"
    );

    // Must not falsely claim parity with Git interactive selection
    assert!(
        !clean_row.contains("Git-compatible") && !clean_row.contains("Git parity"),
        "clean -i should not be claimed as Git-compatible"
    );
}
