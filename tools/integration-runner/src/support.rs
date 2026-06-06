use std::{fs, io::Write, path::PathBuf, process::Output};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use regex::Regex;
use serde_json::Value;

use crate::runner::{Report, RunContext, ScenarioResult, Totals};

pub(crate) fn ensure_file(path: PathBuf) -> Result<()> {
    if !path.exists() {
        bail!("expected path to exist: {}", path.display());
    }
    Ok(())
}

pub(crate) fn assert_stdout_contains(output: &Output, expected: &str) -> Result<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains(expected) {
        bail!("stdout did not contain {expected:?}: {stdout}");
    }
    Ok(())
}

pub(crate) fn assert_not_contains(output: &Output, unexpected: &str) -> Result<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.contains(unexpected) || stderr.contains(unexpected) {
        bail!("output unexpectedly contained {unexpected:?}: stdout={stdout} stderr={stderr}");
    }
    Ok(())
}

pub(crate) fn stdout_trim(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub(crate) fn assert_lbr_or_text(output: &Output, expected: &str) -> Result<()> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stderr.contains("LBR-") || stderr.contains(expected) || stdout.contains(expected) {
        return Ok(());
    }
    bail!("expected LBR- or {expected:?}; stdout={stdout} stderr={stderr}")
}

pub(crate) fn assert_json_ok(output: &Output, command: &str) -> Result<()> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("parse JSON stdout for {command}: {stdout}"))?;
    if value.get("ok") != Some(&Value::Bool(true)) {
        bail!("JSON envelope ok was not true for {command}: {value}");
    }
    if value.get("data").is_none_or(Value::is_null) {
        bail!("JSON envelope missing data for {command}: {value}");
    }
    Ok(())
}

pub(crate) fn assert_json_error_code(output: &Output, error_code: &str) -> Result<()> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = if !stderr.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    let value: Value = serde_json::from_str(raw)
        .with_context(|| format!("parse JSON error output: stdout={stdout} stderr={stderr}"))?;
    if value.get("ok") != Some(&Value::Bool(false)) {
        bail!("JSON error envelope ok was not false: {value}");
    }
    if value.get("error_code").and_then(Value::as_str) != Some(error_code) {
        bail!("JSON error code was not {error_code}: {value}");
    }
    Ok(())
}

/// Pure derivation of wave3_cleanup per §5.5 (extracted for unit testability + branch coverage).
/// "deleted ..." takes precedence; first "cleanup_required" if no deleted; else "not_run".
/// Only meaningful when waves include 3 (from live runs that arm guard).
pub(crate) fn derive_wave3_cleanup(waves_run: &[u8], results: &[ScenarioResult]) -> String {
    if !waves_run.contains(&3) {
        return "not_run".to_string();
    }
    let mut c = "not_run".to_string();
    for r in results {
        if let Some(cl) = &r.cleanup {
            if cl.starts_with("deleted ") {
                c = cl.clone();
                break;
            } else if cl.contains("cleanup_required") && c == "not_run" {
                c = cl.clone();
            }
        }
    }
    c
}

pub(crate) fn write_report(ctx: &RunContext, results: &[ScenarioResult]) -> Result<()> {
    // §3.6: we only reach here after every command's raw output passed ensure_no_secret_leak.
    // Report (and sidecars) are the first point where redacted content may be materialized.
    // All fields (incl. the new §5 design ones) are written after that check.
    let passed = results.iter().filter(|r| r.status == "passed").count();
    let failed = results.iter().filter(|r| r.status == "failed").count();
    let skipped = results.iter().filter(|r| r.status == "skipped").count();
    let finished_at = Utc::now().to_rfc3339();
    // generated_at (legacy) intentionally uses a fresh now() here for "report materialization"
    // time (pre-existing); it will be a few ms after finished_at. We keep both for compat
    // (additive fields only). See §5.7 skew note.

    // Use pure helper (covers "deleted", "cleanup_required", "not_run" branches in unit tests).
    let wave3_cleanup = derive_wave3_cleanup(&ctx.waves_run, results);
    // Current runner behavior (tempdir + .keep(), no post-success rm) always preserves
    // the run_root containing the report. See §5.5 run_root_state and keep flag notes.
    let run_root_state = "preserved".to_string();

    // One to_vec + clone for the alias (was two independent to_vec clones).
    let results_vec = results.to_vec();
    let report = Report {
        // Design §5.5 fields (additive; see types.rs for compat rationale).
        run_id: ctx.run_id.clone(),
        commit: ctx.commit.clone(),
        started_at: ctx.started_at.clone(),
        finished_at: finished_at.clone(),
        waves_run: ctx.waves_run.clone(),
        wave3_cleanup: wave3_cleanup.clone(),
        run_root_state: run_root_state.clone(),

        generated_at: Utc::now().to_rfc3339(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        run_root: ctx.run_root.display().to_string(),
        binary: ctx.binary.display().to_string(),
        // Reaching write_report means every command passed ensure_no_secret_leak
        // before its (redacted) logs were written; the run is leak-clean (§3.6).
        redaction_self_check: "clean".to_string(),
        totals: Totals {
            pass: passed,
            fail: failed,
            skip: skipped,
            env_skip: 0,
            block: 0,
        },
        passed,
        failed,
        skipped,
        results: results_vec.clone(),
        scenarios: results_vec,
    };
    fs::write(
        ctx.run_root.join("report.json"),
        serde_json::to_string_pretty(&report).context("serialize report")?,
    )?;

    let mut ndjson = fs::File::create(&ctx.results_path).context("create results.ndjson")?;
    for result in results {
        writeln!(ndjson, "{}", serde_json::to_string(result)?)?;
    }

    let mut summary = String::new();
    summary.push_str("# Libra Integration Runner Summary\n\n");
    // Include design §5 fields for human-readable summary (run_id/commit/times/waves/state/cleanup).
    summary.push_str(&format!("- run_id: `{}`\n", ctx.run_id));
    summary.push_str(&format!("- commit: `{}`\n", ctx.commit));
    summary.push_str(&format!("- started_at: `{}`\n", ctx.started_at));
    summary.push_str(&format!("- finished_at: `{}`\n", finished_at));
    summary.push_str(&format!("- waves_run: {:?}\n", ctx.waves_run));
    summary.push_str(&format!("- run_root: `{}`\n", ctx.run_root.display()));
    summary.push_str(&format!("- binary: `{}`\n", ctx.binary.display()));
    summary.push_str(&format!(
        "- platform: `{}-{}`\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    summary.push_str(&format!("- run_root_state: {}\n", run_root_state));
    summary.push_str(&format!("- wave3_cleanup: {}\n", wave3_cleanup));
    summary.push_str("- redaction_self_check: clean\n");
    summary.push_str(&format!(
        "- passed: {passed}\n- failed: {failed}\n- skipped: {skipped}\n\n"
    ));
    for result in results {
        summary.push_str(&format!("- `{}`: {}\n", result.id, result.status));
        if result.id.starts_with("live.")
            && let Some(c) = &result.cleanup
        {
            summary.push_str(&format!("  cleanup: {}\n", c));
        }
    }
    fs::write(ctx.run_root.join("summary.md"), summary)?;

    let failures: Vec<_> = results.iter().filter(|r| r.status == "failed").collect();
    let mut failures_md = String::from("# Failures\n\n");
    let mut rerun = String::new();
    for failure in failures {
        failures_md.push_str(&format!(
            "## `{}`\n\n- run_dir: `{}`\n- error: `{}`\n\n",
            failure.id,
            failure.run_dir,
            failure.error.as_deref().unwrap_or("unknown")
        ));
        rerun.push_str(&failure.id);
        rerun.push('\n');
    }
    fs::write(ctx.run_root.join("failures.md"), failures_md)?;
    fs::write(ctx.run_root.join("rerun-failed.txt"), rerun)?;
    Ok(())
}

pub(crate) fn tail(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    text.chars().skip(char_count - max_chars).collect()
}

pub(crate) fn redact(input: &str) -> String {
    let patterns = [
        r"ghp_[A-Za-z0-9]{20,}",
        r"github_pat_[A-Za-z0-9_]{20,}",
        r"AKIA[0-9A-Z]{16}",
        r"xox[baprs]-[A-Za-z0-9-]+",
        r"https?://[^/\s]*:[^/@\s]+@",
        r"gho_[A-Za-z0-9]{20,}", // oauth
    ];
    patterns.iter().fold(input.to_string(), |acc, pattern| {
        Regex::new(pattern)
            .map(|re| re.replace_all(&acc, "[REDACTED]").into_owned())
            .unwrap_or(acc)
    })
}

fn contains_secret(text: &str) -> bool {
    let patterns = [
        r"ghp_[A-Za-z0-9]{20,}",
        r"github_pat_[A-Za-z0-9_]{20,}",
        r"AKIA[0-9A-Z]{16}",
        r"xox[baprs]-[A-Za-z0-9-]+",
        r"https?://[^/\s]*:[^/@\s]+@",
        r"gho_[A-Za-z0-9]{20,}",
    ];
    patterns.iter().any(|pattern| {
        Regex::new(pattern)
            .map(|re| re.is_match(text))
            .unwrap_or(false)
    })
}

/// Check raw output for secrets and bail (before any redaction/write) to prevent leak.
pub(crate) fn ensure_no_secret_leak(
    seq: usize,
    id: &str,
    stdout: &str,
    stderr: &str,
) -> Result<()> {
    if contains_secret(stdout) || contains_secret(stderr) {
        bail!(
            "SECRET LEAK DETECTED in scenario {} seq {} (stdout/stderr matched credential pattern)",
            id,
            seq
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::runner::{RunContext, ScenarioResult};

    fn make_test_ctx(run_root: PathBuf) -> RunContext {
        RunContext {
            run_root: run_root.clone(),
            binary: PathBuf::from("target/debug/libra"),
            safe_path: "/bin:/usr/bin".into(),
            results_path: run_root.join("results.ndjson"),
            run_id: "test-run-123".into(),
            commit: "abc1234".into(),
            started_at: "2026-06-05T00:00:00Z".into(),
            waves_run: vec![0, 1],
        }
    }

    fn make_result(id: &str, status: &str, cleanup: Option<&str>) -> ScenarioResult {
        ScenarioResult {
            id: id.into(),
            wave: if id.starts_with("live.") { 3 } else { 1 },
            status: status.into(),
            duration_ms: 10,
            run_dir: "/tmp/fake".into(),
            commands: vec![],
            error: None,
            cleanup: cleanup.map(|s| s.into()),
        }
    }

    #[test]
    fn test_derive_wave3_cleanup_branches() {
        // Covers all derivation paths for wave3_cleanup (addresses branch coverage gap).
        let no3: Vec<u8> = vec![0, 1];
        let with3: Vec<u8> = vec![0, 1, 3];
        let r_deleted = make_result("live.foo", "passed", Some("deleted owner/repo1"));
        let r_required = make_result("live.bar", "failed", Some("cleanup_required owner/repo2"));
        let r_pass = make_result("cli.baz", "passed", None);

        assert_eq!(derive_wave3_cleanup(&no3, &[]), "not_run");
        assert_eq!(derive_wave3_cleanup(&with3, &[]), "not_run");
        assert_eq!(
            derive_wave3_cleanup(&with3, std::slice::from_ref(&r_deleted)),
            "deleted owner/repo1"
        );
        // deleted wins even if required also present
        assert_eq!(
            derive_wave3_cleanup(&with3, &[r_required.clone(), r_deleted.clone()]),
            "deleted owner/repo1"
        );
        assert_eq!(
            derive_wave3_cleanup(&with3, std::slice::from_ref(&r_required)),
            "cleanup_required owner/repo2"
        );
        assert_eq!(derive_wave3_cleanup(&with3, &[r_pass]), "not_run");
    }

    #[test]
    fn test_write_report_emits_full_additive_shape_and_legacy() {
        // Exercises write_report + metadata threading + alias + legacy keys (addresses
        // lack of automated Report contract tests; only samples previously).
        let td = tempdir().expect("temp run root for report test");
        let mut ctx = make_test_ctx(td.path().to_path_buf());
        ctx.waves_run = vec![0, 1, 3]; // to exercise wave3 path

        let results = vec![
            make_result("cli.init-basic", "passed", None),
            make_result(
                "live.github-foo",
                "failed",
                Some("cleanup_required owner/tmp"),
            ),
        ];

        write_report(&ctx, &results).expect("write_report in test");

        let report_path = td.path().join("report.json");
        assert!(report_path.exists(), "report.json must be written");

        let val: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();

        // New design fields (additive)
        assert!(val.get("run_id").is_some());
        assert!(val.get("commit").is_some());
        assert!(val.get("started_at").is_some());
        assert!(val.get("finished_at").is_some());
        assert!(val.get("waves_run").is_some());
        assert_eq!(val["waves_run"], serde_json::json!([0, 1, 3]));
        assert!(val.get("wave3_cleanup").is_some());
        assert_eq!(val["wave3_cleanup"], "cleanup_required owner/tmp"); // from the result
        assert!(val.get("run_root_state").is_some());
        assert_eq!(val["run_root_state"], "preserved");

        // Legacy + compat preserved
        assert!(val.get("generated_at").is_some());
        assert!(val.get("platform").is_some());
        assert!(val.get("run_root").is_some());
        assert!(val.get("binary").is_some());
        assert!(val.get("redaction_self_check").is_some());
        assert!(val.get("totals").is_some());
        assert!(val.get("passed").is_some());
        assert!(val.get("failed").is_some());
        assert!(val.get("skipped").is_some());
        assert!(val.get("results").is_some());

        // Alias for design model
        assert!(val.get("scenarios").is_some());
        assert_eq!(val["results"], val["scenarios"]);

        // summary also written with new fields
        let summary = std::fs::read_to_string(td.path().join("summary.md")).unwrap();
        assert!(summary.contains("run_id: `test-run-123`"));
        assert!(summary.contains("wave3_cleanup: cleanup_required owner/tmp"));
        assert!(summary.contains("run_root_state: preserved"));
    }
}
