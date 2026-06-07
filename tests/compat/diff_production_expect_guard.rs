//! Production `.expect()` / `.unwrap()` audit for the `libra diff` Wave 0–3 work
//! (v0.17.1341–1346) and its shared helpers.
//!
//! The tree-wide `compat_all_production_unwrap_guard` only catches bare
//! `.unwrap()`. The diff improvement plan additionally requires that every
//! production `.expect(...)` in the files it touched carries an `// INVARIANT:`
//! justification (inline or on the immediately preceding line), so a future
//! edit cannot quietly introduce an unjustified panic site. This guard enforces
//! both: no bare `.unwrap()`, and every `.expect(` is INVARIANT-documented.
//!
//! Scope is the production `src/**` files the plan introduced or restructured:
//! `src/command/diff.rs` and the shared `src/utils/blob_similarity.rs`.

use std::{fs, path::PathBuf};

const TARGET_FILES: &[&str] = &["src/command/diff.rs", "src/utils/blob_similarity.rs"];

#[test]
fn diff_production_code_has_no_unjustified_panics() {
    let mut offenders = Vec::new();

    for &relative in TARGET_FILES {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read '{}': {err}", path.display()));

        let lines: Vec<&str> = text.lines().collect();
        let mut in_tests_module = false;

        for (idx, raw_line) in lines.iter().enumerate() {
            let line_number = idx + 1;
            let trimmed = raw_line.trim_start();

            // Everything from the first `#[cfg(test)]` onward is test code.
            if trimmed.starts_with("#[cfg(test)]") {
                in_tests_module = true;
            }
            if in_tests_module || trimmed.starts_with("//") {
                continue;
            }

            if trimmed.contains(".unwrap()") {
                offenders.push(format!(
                    "{relative}:{line_number} uses a bare `.unwrap()` in production; \
                     return a `Result` or use `.expect(\"INVARIANT: ...\")`"
                ));
            }

            if trimmed.contains(".expect(") {
                let prev = idx
                    .checked_sub(1)
                    .map(|p| lines[p].trim_start())
                    .unwrap_or("");
                let justified = trimmed.contains("// INVARIANT:") || prev.contains("// INVARIANT:");
                if !justified {
                    offenders.push(format!(
                        "{relative}:{line_number} has a production `.expect(...)` without an \
                         `// INVARIANT:` comment (inline or on the preceding line)"
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "diff production code must avoid bare `.unwrap()` and justify every `.expect(...)`:\n{}",
        offenders.join("\n"),
    );
}
