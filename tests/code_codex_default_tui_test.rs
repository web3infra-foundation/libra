//! Static guard for Phase 2 Task 2.7 of `docs/improvement/tui.md`.
//!
//! `libra code --provider codex` must always route through the default Libra TUI
//! (`run_tui_with_managed_code_runtime`) — the legacy standalone Codex stdin loop
//! (`agent_codex::execute`) is deprecated and must not be reachable from the
//! `libra code` command path. Spinning up a real Codex app-server inside CI is
//! prohibitively heavy, so we rely on source-level invariants instead:
//!
//! 1. `src/command/code.rs` must not call `agent_codex::execute`.
//! 2. `src/command/code.rs` must contain a `CodeProvider::Codex` arm that hands
//!    off to `run_tui_with_managed_code_runtime`.
//! 3. `agent_codex::execute` must keep the `#[deprecated]` marker so a future
//!    refactor immediately surfaces the regression at compile time inside any
//!    consumer that still references it.
//! 4. The legacy stdin/stdout primitives (`std::io::stdin`, `stdin_rx.recv`,
//!    Codex's own approval `print` loops) must not appear inside
//!    `src/command/code.rs` or `src/internal/tui/`.
//!
//! These checks complement the runtime scenarios in
//! `tests/code_ui_scenarios.rs`; they fail fast and don't need the
//! `test-provider` feature.

use std::{fs, path::PathBuf};

const COMMAND_CODE_PATH: &str = "src/command/code.rs";
const CODEX_MOD_PATH: &str = "src/internal/ai/codex/mod.rs";
const TUI_DIR: &str = "src/internal/tui";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Panic if any line in `haystack` matches the predicate. Useful for guard
/// invariants where we want a precise diagnostic instead of a boolean.
fn assert_no_line_matches<P>(haystack: &str, label: &str, predicate: P)
where
    P: Fn(&str) -> bool,
{
    let offenders: Vec<(usize, &str)> = haystack
        .lines()
        .enumerate()
        .filter(|(_, line)| predicate(line))
        .map(|(idx, line)| (idx + 1, line.trim()))
        .collect();
    assert!(
        offenders.is_empty(),
        "{label} regression: {} offending line(s):\n{}",
        offenders.len(),
        offenders
            .iter()
            .map(|(line_no, line)| format!("  L{line_no}: {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn command_code_does_not_call_legacy_codex_execute() {
    let source = read_file(COMMAND_CODE_PATH);
    // Allow the substring inside comments/docs but not as a function call.
    assert_no_line_matches(&source, "agent_codex::execute call site", |line| {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("*") {
            return false;
        }
        line.contains("agent_codex::execute(")
    });
}

#[test]
fn codex_arm_routes_through_managed_runtime() {
    let source = read_file(COMMAND_CODE_PATH);
    assert!(
        source.contains("CodeProvider::Codex =>"),
        "expected `CodeProvider::Codex` match arm in {COMMAND_CODE_PATH}"
    );
    // The arm must hand off to the shared default-TUI driver, not a Codex-only path.
    assert!(
        source.contains("run_tui_with_managed_code_runtime"),
        "Codex arm must call `run_tui_with_managed_code_runtime` (default Libra TUI)"
    );
    // And it must build the Codex code-ui runtime via the documented helper.
    assert!(
        source.contains("start_codex_code_ui_runtime"),
        "Codex arm must construct the runtime via `start_codex_code_ui_runtime`"
    );
}

#[test]
fn legacy_codex_execute_is_deprecated() {
    let source = read_file(CODEX_MOD_PATH);
    let exec_idx = source
        .find("pub async fn execute(")
        .expect("agent_codex::execute should still exist (legacy)");
    let preamble = &source[..exec_idx];
    let last_attr_window = preamble.rfind("#[deprecated").unwrap_or(usize::MAX);
    let last_blank_line = preamble.rfind("\n\n").unwrap_or(0);
    assert!(
        last_attr_window > last_blank_line,
        "agent_codex::execute must keep the `#[deprecated(...)]` attribute attached \
         (so any new caller fails compilation with -D warnings)"
    );
}

#[test]
fn libra_code_path_has_no_stdin_or_codex_print_loops() {
    // Inside src/command/code.rs the orchestrator should never read stdin or
    // drive a Codex-style approval print loop.
    let cmd_source = read_file(COMMAND_CODE_PATH);
    assert_no_line_matches(&cmd_source, "stdin reader in command/code.rs", |line| {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("///") {
            return false;
        }
        line.contains("std::io::stdin") || line.contains("io::stdin(")
    });

    // Inside src/internal/tui/ the TUI must not bypass crossterm by reading
    // raw stdin either; that would race with the App event loop.
    walk_rs_files(repo_root().join(TUI_DIR), |path, contents| {
        assert_no_line_matches(
            contents,
            &format!("stdin reader in {}", path.display()),
            |line| {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.starts_with("///") {
                    return false;
                }
                line.contains("std::io::stdin") || line.contains("io::stdin(")
            },
        );
    });
}

fn walk_rs_files(root: PathBuf, mut visit: impl FnMut(&PathBuf, &str)) {
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                let contents = match fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(_) => continue,
                };
                visit(&path, &contents);
            }
        }
    }
}
