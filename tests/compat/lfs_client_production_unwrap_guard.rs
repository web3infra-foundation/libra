//! Defensive guard for the v0.17.237 / v0.17.254-258 production unwrap audit
//! in `src/internal/protocol/lfs_client.rs`.
//!
//! Every production call site in that file has been migrated to use
//! `.expect("...")` with an INVARIANT comment that names the broken contract.
//! This test scans the file and fails the build if a contributor reintroduces
//! a bare `.unwrap()` outside the `#[cfg(test)] mod tests` block.
//!
//! The scan is intentionally narrow:
//!   - It only inspects the production region (everything before the
//!     `#[cfg(test)]` attribute that delimits the inline tests module).
//!   - Lines starting with `//` (line comments, including triple-slash
//!     rustdoc) and lines inside common doc-example fences are skipped.
//!   - Whitespace-tolerant: a bare `.unwrap()` with any preceding spaces
//!     still trips the guard.

use std::{fs, path::PathBuf};

/// The single file this guard protects. If the file moves, update this
/// constant alongside the move so the guard keeps surfacing regressions
/// at compile-time.
const TARGET_FILE: &str = "src/internal/protocol/lfs_client.rs";

#[test]
fn lfs_client_production_has_no_bare_unwrap_calls() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(TARGET_FILE);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read '{}': {err}", path.display()));

    let mut in_tests_module = false;
    let mut offenders = Vec::new();

    for (idx, raw_line) in text.lines().enumerate() {
        let line_number = idx + 1;
        let trimmed = raw_line.trim_start();

        // Once we cross into the inline tests module, stop scanning;
        // tests legitimately use .unwrap().
        if trimmed.starts_with("#[cfg(test)]") {
            in_tests_module = true;
        }
        if in_tests_module {
            continue;
        }

        // Skip rustdoc and ordinary comments.
        if trimmed.starts_with("//") {
            continue;
        }

        if trimmed.contains(".unwrap()") {
            offenders.push(format!(
                "{TARGET_FILE}:{line_number} reintroduces bare `.unwrap()` in production code: \
                 use `.expect(\"INVARIANT: ...\")` instead (see CLAUDE.md error handling rules)"
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "production code in {TARGET_FILE} must use INVARIANT-documented `.expect(\"...\")` \
         instead of bare `.unwrap()`:\n{}",
        offenders.join("\n"),
    );
}
