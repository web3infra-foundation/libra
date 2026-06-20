//! Guard the optional live compatibility workflow shape.
//!
//! Live AI/cloud gates require external secrets, so they must remain outside
//! the base required-check workflow. This test locks the local contract that
//! `compat-live-*` jobs are manual/scheduled, secret-gated, and absent from
//! `base.yml`.

use std::{fs, path::PathBuf};

#[test]
fn live_compat_workflow_is_optional_and_secret_gated() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let live = fs::read_to_string(repo.join(".github/workflows/live-compat.yml"))
        .expect("read .github/workflows/live-compat.yml");
    let base = fs::read_to_string(repo.join(".github/workflows/base.yml"))
        .expect("read .github/workflows/base.yml");

    for required in [
        "workflow_dispatch: {}",
        "schedule:",
        "name: compat-live-ai",
        "name: compat-live-cloud",
        "DEEPSEEK_API_KEY",
        "LIBRA_D1_ACCOUNT_ID",
        "LIBRA_STORAGE_SECRET_KEY",
        "skip=true",
        "--features test-live-ai",
        "--features test-live-cloud",
    ] {
        assert!(
            live.contains(required),
            "live compatibility workflow is missing expected marker: {required}"
        );
    }

    for forbidden in ["pull_request:", "push:"] {
        assert!(
            !live.contains(forbidden),
            "live compatibility workflow must not run as a required PR/push gate: {forbidden}"
        );
    }

    for live_job in ["compat-live-ai", "compat-live-cloud"] {
        assert!(
            !base.contains(live_job),
            "base.yml required-check workflow must not include optional live job {live_job}"
        );
    }
}
