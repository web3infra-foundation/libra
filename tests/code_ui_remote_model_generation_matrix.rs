//! Wave 11 / PR 11 — `code_ui_remote_model_generation` matrix
//! runner (§5.19, L3, nightly-only).
//!
//! Loads `tests/data/code_ui_remote/model_generation_cases.json`
//! and runs the live-model-generation cases through a real
//! `libra code` PTY session. The matrix harness now supports the
//! `provider.mode == "model_from_env_file"` shape; this runner
//! reads the configured env file (default `.env.test`), resolves
//! `LIBRA_CODE_TEST_PROVIDER` + `LIBRA_CODE_TEST_MODEL`, and
//! spawns `libra code --provider <provider> --model <model>
//! --env-file <path>` so the live API key is loaded from disk.
//!
//! Default behaviour: every case is `#[ignore]` so a normal
//! `cargo test` skips the suite. Run on demand with:
//!
//! ```bash
//! LIBRA_RUN_LIVE=1 cargo test --features test-provider \
//!   --test code_ui_remote_model_generation_matrix \
//!   -- --ignored --test-threads=1
//! ```
//!
//! The runner short-circuits with a clear, actionable error
//! before invoking the matrix when:
//!   * `LIBRA_RUN_LIVE` is unset (so an accidental `--ignored`
//!     run without explicit opt-in fails loud rather than firing
//!     against the model API), or
//!   * the env file is missing / lacks the required keys
//!     (build_session_options panics with the case name).
//!
//! Out-of-scope for this runner (still tracked in the §5.19 doc
//! note): the linked-cargo-cli case carries assertions like
//! `cargo fmt --all --check` + `cargo clippy --all-targets
//! --all-features -- -D warnings` that need a workspace-aware
//! `runRepoCommand` fan-out the JSON case file does not yet
//! describe; the runner accepts both cases but the second one
//! will only validate what `runRepoCommand` already enforces
//! today.

#[cfg(feature = "test-provider")]
mod harness;

#[cfg(feature = "test-provider")]
use anyhow::{Result, bail};
#[cfg(feature = "test-provider")]
use harness::CodeSession;
#[cfg(feature = "test-provider")]
use harness::matrix::{Case, CaseFile, build_session_options, find_case, load_case_file};
#[cfg(feature = "test-provider")]
use serial_test::serial;

#[cfg(feature = "test-provider")]
const CASE_FILE_PATH: &str = "tests/data/code_ui_remote/model_generation_cases.json";

#[cfg(feature = "test-provider")]
fn live_mode_enabled() -> bool {
    std::env::var("LIBRA_RUN_LIVE")
        .ok()
        .as_deref()
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

#[cfg(feature = "test-provider")]
fn run_model_generation_case(case_name: &str) -> Result<()> {
    if !live_mode_enabled() {
        bail!(
            "LIBRA_RUN_LIVE=1 must be set to run live model-generation case '{case_name}'; rerun with `LIBRA_RUN_LIVE=1 cargo test --features test-provider --test code_ui_remote_model_generation_matrix -- --ignored --test-threads=1`",
        );
    }
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
macro_rules! model_generation_case {
    ($name:ident) => {
        #[test]
        #[ignore = "L3 live model; run with LIBRA_RUN_LIVE=1 + .env.test (DEEPSEEK_API_KEY)"]
        #[serial]
        fn $name() -> Result<()> {
            run_model_generation_case(stringify!($name))
        }
    };
}

// Wave 11 — both P0 cases from `model_generation_cases.json`. The
// linked-cargo-cli case will pass through the harness but its
// quality-gate assertions (`cargo fmt`, `cargo clippy`,
// `cargo test`) are bounded by what `runRepoCommand` already
// supports; deeper workflow validation lands in a follow-up.
#[cfg(feature = "test-provider")]
model_generation_case!(model_generation_code_service_creates_tested_rust_file);
#[cfg(feature = "test-provider")]
model_generation_case!(model_generation_linked_cargo_cli_project_passes_quality_gates);

#[cfg(not(feature = "test-provider"))]
#[test]
fn model_generation_matrix_requires_test_provider_feature() {
    eprintln!("skipping model generation matrix; enable --features test-provider");
}
