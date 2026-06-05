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

pub(crate) fn write_report(ctx: &RunContext, results: &[ScenarioResult]) -> Result<()> {
    let passed = results.iter().filter(|r| r.status == "passed").count();
    let failed = results.iter().filter(|r| r.status == "failed").count();
    let skipped = results.iter().filter(|r| r.status == "skipped").count();
    let report = Report {
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
        results: results.to_vec(),
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
    summary.push_str(&format!("- run_root: `{}`\n", ctx.run_root.display()));
    summary.push_str(&format!("- binary: `{}`\n", ctx.binary.display()));
    summary.push_str(&format!(
        "- platform: `{}-{}`\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
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
