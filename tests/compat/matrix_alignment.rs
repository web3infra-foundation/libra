//! `tests/compat/matrix_alignment.rs` — drift detection for public
//! compatibility/documentation matrices.
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

#[test]
fn docs_consistency_script_covers_code_ui_router_matrix() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let web_mod = std::fs::read_to_string(repo.join("src/internal/ai/web/mod.rs"))
        .expect("read src/internal/ai/web/mod.rs");
    let script = std::fs::read_to_string(repo.join("scripts/check_docs_consistency.sh"))
        .expect("read scripts/check_docs_consistency.sh");

    let router_region = web_mod
        .split("fn code_router()")
        .nth(1)
        .expect("code_router function exists")
        .split("async fn static_handler")
        .next()
        .expect("static_handler follows code routers");

    let mut routes = router_region
        .lines()
        .filter_map(|line| {
            let start = line.find(".route(\"")? + ".route(\"".len();
            let rest = &line[start..];
            let end = rest.find('"')?;
            Some(format!("/api/code{}", &rest[..end]))
        })
        .collect::<Vec<_>>();
    routes.sort();
    routes.dedup();

    assert!(
        !routes.is_empty(),
        "expected to extract /api/code routes from src/internal/ai/web/mod.rs"
    );

    for route in routes {
        assert!(
            script.contains(&route),
            "scripts/check_docs_consistency.sh must require docs for {route}"
        );
    }
}

#[test]
fn web_build_job_checks_static_export_drift() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workflow = std::fs::read_to_string(repo.join(".github/workflows/base.yml"))
        .expect("read .github/workflows/base.yml");
    assert!(
        workflow.contains("bash scripts/check_web_out_clean.sh"),
        "web build CI job must run scripts/check_web_out_clean.sh after pnpm build"
    );

    let script = std::fs::read_to_string(repo.join("scripts/check_web_out_clean.sh"))
        .expect("read scripts/check_web_out_clean.sh");
    assert!(
        script.contains("git status --porcelain -- web/out"),
        "web/out drift script must detect modified and untracked static export files"
    );
}

#[test]
fn lfs_compatibility_docs_use_current_attributes_filename() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for path in [
        "COMPATIBILITY.md",
        "docs/improvement/compatibility/declined.md",
        "docs/improvement/compatibility/governance.md",
    ] {
        let body = std::fs::read_to_string(repo.join(path)).unwrap_or_else(|error| {
            panic!("read {path}: {error}");
        });
        assert!(
            body.contains(".libra_attributes"),
            "{path} must mention the current Libra attributes filename"
        );
        assert!(
            !body.contains(".libraattributes"),
            "{path} must not mention the retired .libraattributes spelling"
        );
    }
}
