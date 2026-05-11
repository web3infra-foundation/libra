//! Wave 7 / PR 7 — `code_ui_remote_security` matrix runner.
//!
//! Loads `tests/data/code_ui_remote/security_cases.json` and runs
//! the L2-driven P1 cases through a real `libra code` PTY session:
//!
//! 1. diagnostics body never echoes either the harness's
//!    `X-Libra-Control-Token` value or the issued
//!    `controllerToken`,
//! 2. diagnostics redacts secret-like substrings from
//!    `LIBRA_LOG_FILE` (driven via per-case `extraEnv`),
//! 3. `--control observe` rejects an automation attach with
//!    403 / `CONTROL_DISABLED`,
//! 4. `/threads?limit=abc` returns 400 / `INVALID_QUERY_PARAM`,
//! 5. `/threads?limit=99999` clamps to ≤200 items,
//! 6. control audit log redacts secret-like client ids on attach.
//!
//! The two `testKind: inline` cases in the JSON file
//! (`security_non_loopback_session_route_is_inline_unit` and
//! `security_non_loopback_messages_route_rejects_before_controller_token`)
//! are intentionally NOT mapped here — they are inline `#[test]`s
//! in `src/internal/ai/web/mod.rs` (added by PR 2 / Wave 2) and
//! the JSON entry is only a marker that those scenarios exist
//! under inline coverage.

#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use anyhow::Result;
#[cfg(feature = "test-provider")]
use harness::CodeSession;
#[cfg(feature = "test-provider")]
use harness::matrix::{Case, CaseFile, build_session_options, find_case, load_case_file};
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
const CASE_FILE_PATH: &str = "tests/data/code_ui_remote/security_cases.json";

#[cfg(feature = "test-provider")]
fn run_security_case(case_name: &str) -> Result<()> {
    let file_path = harness::matrix::data_path(CASE_FILE_PATH);
    let file: CaseFile = load_case_file(&file_path)?;
    let case: Case = find_case(&file, case_name)?;
    let options = build_session_options(&file, &case);
    let mut session = CodeSession::spawn(options)?;
    let outcome = harness::matrix::run_case(&mut session, &case);
    let shutdown = session.shutdown();
    outcome?;
    shutdown
}

#[cfg(feature = "test-provider")]
macro_rules! security_case {
    ($name:ident) => {
        #[test]
        #[serial]
        fn $name() -> Result<()> {
            run_security_case(stringify!($name))
        }
    };
}

// Wave 7 — six L2 P1 cases. The two inline-only entries in the
// JSON file are covered by the existing route-level inline tests
// landed in Wave 2 (`src/internal/ai/web/mod.rs mod tests`).
#[cfg(feature = "test-provider")]
security_case!(security_diagnostics_does_not_expose_control_or_controller_token);
#[cfg(feature = "test-provider")]
security_case!(security_diagnostics_redacts_secret_like_log_file_path);
#[cfg(feature = "test-provider")]
security_case!(security_attach_with_control_observe_is_403_control_disabled);
#[cfg(feature = "test-provider")]
security_case!(security_threads_invalid_limit_returns_invalid_query_param);
#[cfg(feature = "test-provider")]
security_case!(security_threads_limit_clamped_to_200_max);
#[cfg(feature = "test-provider")]
security_case!(security_audit_log_records_attach_with_redacted_client_id);

#[cfg(not(feature = "test-provider"))]
#[test]
fn security_matrix_requires_test_provider_feature() {
    eprintln!("skipping security matrix; enable --features test-provider");
}
