//! `tests/compat/matrix_alignment.rs` — drift detection for the
//! `COMPATIBILITY.md` top-level command matrix.
//!
//! The compatibility matrix promises to cover every top-level
//! `src/cli.rs::Commands` variant. This test runs the same script used by
//! CI so `cargo test --all` catches command additions or removals that forget
//! to update the public matrix.

use std::{path::PathBuf, process::Command};

#[test]
fn compatibility_matrix_matches_cli_commands() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("bash")
        .arg("scripts/check_compat_matrix.sh")
        .current_dir(&repo)
        .output()
        .expect("run scripts/check_compat_matrix.sh");

    assert!(
        output.status.success(),
        "compatibility matrix drift check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
