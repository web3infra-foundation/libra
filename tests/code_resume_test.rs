//! Wave 9 / PR 9 — `libra code --resume <thread_id>` CLI surface
//! coverage (§5.16, partial).
//!
//! What this test pins:
//!   * The `--resume <uuid>` CLI flag round-trips through
//!     `SessionStore::load_for_thread_id`. Passing a syntactically
//!     valid UUID for a session that does not exist surfaces the
//!     documented "no Libra Code session found …" error and the
//!     process exits with a non-zero status.
//!   * `--resume <bad-format>` surfaces the documented
//!     "expects a canonical thread_id UUID" error.
//!   * The harness extensions
//!     (`CodeSessionOptions::with_existing_repo_dir`,
//!     `CodeSessionOptions::with_resume_thread`) drive the spawn
//!     with the correct argv shape so the broader resume contract
//!     can build on the same primitives once a session-binding
//!     entry point is wired through the fake provider path.
//!
//! Both validation cases drive a real `libra code` invocation
//! through a PTY so the runtime's terminal-init guard does not
//! short-circuit — that's why we use the existing
//! `harness::CodeSession::spawn` path (which provisions a PTY
//! pair) and capture the failure mode through the artifact dir
//! the harness writes for every spawn.
//!
//! What this test does NOT cover (deferred):
//!   * The full happy-path "spawn → submit → shutdown → resume →
//!     transcript present" scenario from §5.16. The chat-path
//!     submit (`/run …`) the harness drives does NOT bind a
//!     canonical thread — `apply_thread_bundle_to_snapshot` only
//!     fires when the planning workflow allocates a canonical
//!     thread_id, and the snapshot's `sessionId` for a chat-only
//!     session is the runtime's millisecond-id generator output
//!     (`19e…-…-0000`), NOT a UUID. `code.rs:2634` rejects any
//!     `--resume` value that isn't UUID-shaped, so the second
//!     spawn fails before the SessionStore lookup. Closing the
//!     gap requires either a fake provider path that triggers a
//!     plan workflow (and therefore a canonical thread bind), or
//!     a runtime change that lets `--resume` accept the chat
//!     session's millisecond id. Both are larger than this PR.
//!   * The §5.16 SIGTERM-mid-turn case — would race the
//!     synchronous fake provider's assistant flush.

#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use std::{path::PathBuf, process::Command};

#[cfg(feature = "test-provider")]
use anyhow::{Context, Result, bail};
#[cfg(feature = "test-provider")]
use harness::{CodeSession, CodeSessionOptions};
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
const FIXTURE: &str = "tests/fixtures/code_ui/basic_chat.json";

#[cfg(feature = "test-provider")]
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

#[cfg(feature = "test-provider")]
fn libra_bin_path() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_libra")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_libra is set for integration tests")
}

#[cfg(feature = "test-provider")]
fn run_libra_init(repo_dir: &std::path::Path) -> Result<()> {
    let output = Command::new(libra_bin_path())
        .args(["init", "--vault=false", "--quiet"])
        .arg(repo_dir)
        .output()
        .context("failed to run 'libra init'")?;
    if !output.status.success() {
        bail!(
            "libra init failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Spawn `libra code` through the harness PTY with the supplied
/// `--resume <thread_id>` value, expect the spawn to fail
/// because the runtime exits before writing control info, and
/// return the captured pty.log + libra.log so the caller can
/// pin the documented failure message.
#[cfg(feature = "test-provider")]
fn expect_resume_spawn_failure(case_name: &str, thread_id: &str) -> Result<(String, String)> {
    let repo_root = tempfile::Builder::new()
        .prefix(&format!("{case_name}-"))
        .tempdir()
        .context("failed to create resume tempdir")?;
    let repo_dir = repo_root.path().join("repo");
    std::fs::create_dir_all(&repo_dir).context("failed to create repo subdir")?;
    run_libra_init(&repo_dir)?;

    let result = CodeSession::spawn(
        CodeSessionOptions::new(case_name, fixture_path())
            .with_existing_repo_dir(repo_dir.clone())
            .with_resume_thread(thread_id),
    );
    match result {
        Ok(_session) => {
            bail!("expected --resume {thread_id} to fail spawn, but a session was created")
        }
        Err(_err) => {}
    }
    let logs_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("code-ui-scenarios")
        .join(case_name);
    let pty_log = std::fs::read_to_string(logs_dir.join("pty.log")).unwrap_or_default();
    let libra_log = std::fs::read_to_string(logs_dir.join("libra.log")).unwrap_or_default();
    Ok((pty_log, libra_log))
}

/// Pin the `--resume <uuid>` failure path: a syntactically valid
/// UUID that does not match any persisted session must surface
/// the documented "no Libra Code session found …" error. Driving
/// this from a real `libra code` PTY (rather than calling
/// `SessionStore` directly) covers the CLI argv plumbing all the
/// way down to `SessionStore::load_for_thread_id`.
#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn resume_with_unknown_uuid_thread_id_surfaces_session_not_found() -> Result<()> {
    let phantom_thread_id = "00000000-1111-2222-3333-444455556666";
    let (pty, libra) = expect_resume_spawn_failure("code-resume-unknown-uuid", phantom_thread_id)?;
    let combined = format!("{pty}\n{libra}");
    assert!(
        combined.contains("no Libra Code session found") && combined.contains(phantom_thread_id),
        "combined logs did not include the documented session-not-found error;\npty.log:\n{pty}\nlibra.log:\n{libra}"
    );
    Ok(())
}

/// Pin the `--resume <bad-format>` validation path: any
/// non-UUID value must surface the documented
/// "expects a canonical thread_id UUID" error.
#[cfg(feature = "test-provider")]
#[test]
#[serial]
fn resume_with_non_uuid_thread_id_rejects_with_canonical_uuid_error() -> Result<()> {
    let bad_thread_id = "not-a-uuid-at-all";
    let (pty, libra) = expect_resume_spawn_failure("code-resume-bad-format", bad_thread_id)?;
    let combined = format!("{pty}\n{libra}");
    assert!(
        combined.contains("--resume expects a canonical thread_id UUID")
            && combined.contains(bad_thread_id),
        "combined logs did not include the documented validation error;\npty.log:\n{pty}\nlibra.log:\n{libra}"
    );
    Ok(())
}

#[cfg(not(feature = "test-provider"))]
#[test]
fn resume_test_requires_test_provider_feature() {
    eprintln!("skipping resume test; enable --features test-provider");
}
