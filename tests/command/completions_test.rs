//! Integration tests for `libra completions`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network, no repo).

use tempfile::tempdir;

use super::{parse_json_stdout, run_libra_command};

/// Every supported shell should produce a non-empty completion script and
/// succeed even outside a repository.
#[test]
fn completions_generate_nonempty_scripts_for_every_shell() {
    let dir = tempdir().unwrap();
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let result = run_libra_command(&["completions", shell], dir.path());
        assert_eq!(
            result.status.code(),
            Some(0),
            "`completions {shell}` should succeed; stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !result.stdout.is_empty(),
            "`completions {shell}` should emit a non-empty script"
        );
    }
}

/// The bash script should mention the `libra` binary name.
#[test]
fn completions_bash_mentions_libra() {
    let dir = tempdir().unwrap();
    let result = run_libra_command(&["completions", "bash"], dir.path());
    assert_eq!(result.status.code(), Some(0));
    let script = String::from_utf8_lossy(&result.stdout);
    assert!(
        script.contains("libra"),
        "bash completion script should mention `libra`"
    );
}

/// `--json completions <shell>` wraps the script in a `{ shell, script }`
/// envelope.
#[test]
fn completions_json_envelope() {
    let dir = tempdir().unwrap();
    let result = run_libra_command(&["--json", "completions", "zsh"], dir.path());
    assert_eq!(result.status.code(), Some(0));
    let json = parse_json_stdout(&result);
    assert_eq!(json["data"]["shell"].as_str(), Some("zsh"));
    assert!(
        json["data"]["script"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "JSON envelope should carry a non-empty script"
    );
}

/// An unknown shell is a clap usage error (Git-style exit 129), not a panic.
#[test]
fn completions_unknown_shell_is_usage_error() {
    let dir = tempdir().unwrap();
    let result = run_libra_command(&["completions", "tcsh"], dir.path());
    assert_eq!(result.status.code(), Some(129));
}

/// A missing shell argument is a clap usage error (Git-style exit 129).
#[test]
fn completions_missing_shell_is_usage_error() {
    let dir = tempdir().unwrap();
    let result = run_libra_command(&["completions"], dir.path());
    assert_eq!(result.status.code(), Some(129));
}
