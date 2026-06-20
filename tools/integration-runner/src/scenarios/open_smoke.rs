use std::process::Output;

use serde_json::Value;

use super::prelude::*;

const ORIGIN_URL: &str = "git@github.com:example/open-repo.git";

pub(crate) fn scenario_open_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    ctx.command(&["remote", "add", "origin", ORIGIN_URL], repo.clone(), true)?;

    assert_open_json(
        &ctx.command(&["--json", "open"], repo.clone(), true)?,
        Some("origin"),
        ORIGIN_URL,
        "https://github.com/example/open-repo",
    )?;
    assert_open_json(
        &ctx.command(&["--json", "open", "origin"], repo.clone(), true)?,
        Some("origin"),
        ORIGIN_URL,
        "https://github.com/example/open-repo",
    )?;
    assert_open_json(
        &ctx.command(
            &["--json", "open", "https://github.com/example/direct"],
            repo.clone(),
            true,
        )?,
        None,
        "https://github.com/example/direct",
        "https://github.com/example/direct",
    )?;

    let branch_open = ctx.command(
        &["--json", "open", "-b", "main", "origin"],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&branch_open, "LBR-CLI-002")?;
    let print_only = ctx.command(&["open", "--print-only", "origin"], repo.clone(), false)?;
    assert_lbr_or_text(&print_only, "--print-only")?;

    let bad_open = ctx.command(
        &["--json", "open", "nonexistent-remote"],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&bad_open, "LBR-CLI-003")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}

fn assert_open_json(
    output: &Output,
    remote: Option<&str>,
    remote_url: &str,
    web_url: &str,
) -> Result<()> {
    assert_json_ok(output, "open")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("parse open JSON: {stdout}"))?;
    let data = value
        .get("data")
        .context("open JSON envelope missing data")?;
    if data.get("remote").and_then(Value::as_str) != remote {
        bail!("open JSON remote mismatch: expected {remote:?}, got {data}");
    }
    if data.get("remote_url").and_then(Value::as_str) != Some(remote_url) {
        bail!("open JSON remote_url mismatch: expected {remote_url:?}, got {data}");
    }
    if data.get("web_url").and_then(Value::as_str) != Some(web_url) {
        bail!("open JSON web_url mismatch: expected {web_url:?}, got {data}");
    }
    if data.get("launched").and_then(Value::as_bool) != Some(false) {
        bail!("open JSON launched was not false: {data}");
    }
    Ok(())
}
