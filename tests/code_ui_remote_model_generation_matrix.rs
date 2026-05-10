//! Wave 11 / PR 11 — `code_ui_remote_model_generation` matrix
//! runner skeleton (§5.19, L3, nightly-only). DEFERRED — see
//! the closure note at the bottom of this docstring.
//!
//! This file is a placeholder for the live-model generation
//! matrix described in `tests/data/code_ui_remote/model_generation_cases.json`.
//! The JSON schema requires a NEW provider mode
//! (`provider.mode == "model_from_env_file"`) that the matrix
//! harness does not yet support, plus a `.env.test` file with a
//! valid `DEEPSEEK_API_KEY` to drive the live provider. Wiring
//! both is roadmap-sized work: see §6.4 Wave 4 in
//! `docs/improvement/test.md` for the full design.
//!
//! CLOSURE STATUS: NOT closed. The 12 PR Wave roadmap row 11 is
//! deferred — Codex pass-1 review explicitly flagged that this
//! file is a deferred placeholder, every case is `#[ignore]`,
//! and even with `LIBRA_RUN_LIVE=1` the runner unconditionally
//! bails because `model_from_env_file` is absent from
//! `tests/harness/matrix.rs`. The roadmap stays open on row 11
//! until the harness wiring + nightly CI execution land.
//!
//! What this skeleton DOES today:
//!   * Marks every case as `#[ignore]` so a normal `cargo test`
//!     skips it.
//!   * Gates execution on `LIBRA_RUN_LIVE=1` so even an
//!     `--ignored` run won't accidentally fire against a
//!     production model API in CI without explicit opt-in.
//!   * Fails loud with the deferral message when live mode is
//!     requested but the matrix harness still lacks the
//!     `model_from_env_file` provider plumbing — that prevents
//!     a future operator from believing they ran the suite when
//!     the runner is still a no-op.
//!
//! Bring this runner up to a working state by:
//!   1. Extending `tests/harness/matrix.rs` with a
//!      `ProviderSpec::ModelFromEnvFile { envFile, providerEnv,
//!      modelEnv }` variant alongside the current `FixtureRef`
//!      (which becomes `ProviderSpec::FakeFixture`).
//!   2. Extending `CodeSessionOptions` with `env_file` /
//!      `provider_override` / `model_override` fields and
//!      wiring them into the `libra code` argv.
//!   3. Routing `build_session_options` to honour either the
//!      fake fixture path or the env-file path, depending on
//!      the case's provider mode.

#[cfg(feature = "test-provider")]
use anyhow::{Result, bail};
#[cfg(feature = "test-provider")]
use serial_test::serial;

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
    bail!(
        "model-generation case '{case_name}' requires the `model_from_env_file` provider mode in `tests/harness/matrix.rs`, which is tracked as Wave 11 follow-up scaffolding (§6.4 Wave 4 in docs/improvement/test.md). Skip the runner until the harness supports it.",
    )
}

#[cfg(feature = "test-provider")]
macro_rules! model_generation_case {
    ($name:ident) => {
        #[test]
        #[ignore = "L3 live model; run with LIBRA_RUN_LIVE=1 once harness supports model_from_env_file"]
        #[serial]
        fn $name() -> Result<()> {
            run_model_generation_case(stringify!($name))
        }
    };
}

#[cfg(feature = "test-provider")]
model_generation_case!(model_generation_code_service_creates_tested_rust_file);
#[cfg(feature = "test-provider")]
model_generation_case!(model_generation_linked_cargo_cli_project_passes_quality_gates);

#[cfg(not(feature = "test-provider"))]
#[test]
fn model_generation_matrix_requires_test_provider_feature() {
    eprintln!("skipping model generation matrix; enable --features test-provider");
}
