//! Data-driven controller-lease matrix.
//!
//! Each `#[test]` here loads `tests/data/code_ui_remote/lease_cases.json`
//! and runs the named case end-to-end against a fresh `libra code` PTY
//! session. Adding a new lease scenario is a data-only change in the JSON
//! file plus one one-line entry in this test module.

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
const CASE_FILE_PATH: &str = "tests/data/code_ui_remote/lease_cases.json";

#[cfg(feature = "test-provider")]
fn run_lease_case(case_name: &str) -> Result<()> {
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
macro_rules! lease_case {
    ($name:ident) => {
        #[test]
        #[serial]
        fn $name() -> Result<()> {
            run_lease_case(stringify!($name))
        }
    };
}

#[cfg(feature = "test-provider")]
lease_case!(lease_attach_automation_succeeds_with_control_token);

#[cfg(feature = "test-provider")]
lease_case!(lease_attach_automation_without_control_token_is_403_missing);

#[cfg(feature = "test-provider")]
lease_case!(lease_attach_automation_with_wrong_control_token_is_403_invalid);

#[cfg(feature = "test-provider")]
lease_case!(lease_attach_invalid_kind_returns_400);

#[cfg(not(feature = "test-provider"))]
#[test]
fn lease_matrix_requires_test_provider_feature() {
    eprintln!("skipping lease matrix; enable --features test-provider");
}
