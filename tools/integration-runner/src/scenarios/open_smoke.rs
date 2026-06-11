use std::process::Output;

use serde_json::Value;

use super::prelude::*;

const ORIGIN_URL: &str = "git@github.com:example/open-repo.git";

pub(crate) fn scenario_open_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    ctx.command(&["remote", "add", "origin", ORIGIN_URL], repo.clone(), true)?;

    let default_open = ctx.command(&["--json", "open"], repo.clone(), true)?;
    assert_open_json(
        &default_open,
        "origin",
        "https://github.com/example/open-repo",
        "repo",
        "github",
    )?;
    let origin_open = ctx.command(&["--json", "open", "origin"], repo.clone(), true)?;
    assert_open_json(
        &origin_open,
        "origin",
        "https://github.com/example/open-repo",
        "repo",
        "github",
    )?;
    let branch_open = ctx.command(
        &["--json", "open", "-b", "main", "origin"],
        repo.clone(),
        true,
    )?;
    assert_open_json(
        &branch_open,
        "origin",
        "https://github.com/example/open-repo/tree/main",
        "branch",
        "github",
    )?;

    // --print-only (text mode): prints exactly the resolved URL, never launches.
    let print_only = ctx.command(&["open", "--print-only", "origin"], repo.clone(), true)?;
    let print_only_stdout = stdout_trim(&print_only);
    if print_only_stdout != "https://github.com/example/open-repo" {
        bail!("open --print-only stdout mismatch: got {print_only_stdout:?}");
    }
    assert_not_contains(&print_only, "Opening")?;

    // --pr deep links (mutually exclusive with -b/-c/--issue: separate invocations).
    let pr_open = ctx.command(
        &["--json", "open", "--pr=123", "--print-only", "origin"],
        repo.clone(),
        true,
    )?;
    assert_open_json(
        &pr_open,
        "origin",
        "https://github.com/example/open-repo/pull/123",
        "pull_request",
        "github",
    )?;
    let pr_list = ctx.command(&["--json", "open", "-p", "origin"], repo.clone(), true)?;
    assert_open_json(
        &pr_list,
        "origin",
        "https://github.com/example/open-repo/pulls",
        "pull_request",
        "github",
    )?;

    ctx.command(&["config", "open.platform", "gitlab"], repo.clone(), true)?;
    let gitlab_commit = ctx.command(
        &["--json", "open", "-c", "a1b2c3d", "origin"],
        repo.clone(),
        true,
    )?;
    assert_open_json(
        &gitlab_commit,
        "origin",
        "https://github.com/example/open-repo/-/commit/a1b2c3d",
        "commit",
        "gitlab",
    )?;

    ctx.command(&["config", "open.platform", "custom"], repo.clone(), true)?;
    ctx.command(
        &[
            "config",
            "open.template.issue",
            "{base_url}/tickets/{issue}",
        ],
        repo.clone(),
        true,
    )?;
    let custom_issue = ctx.command(
        &["--json", "open", "--issue=42", "origin"],
        repo.clone(),
        true,
    )?;
    assert_open_json(
        &custom_issue,
        "origin",
        "https://github.com/example/open-repo/tickets/42",
        "issue",
        "custom",
    )?;

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
    remote: &str,
    web_url: &str,
    target_type: &str,
    platform: &str,
) -> Result<()> {
    assert_json_ok(output, "open")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("parse open JSON: {stdout}"))?;
    let data = value
        .get("data")
        .context("open JSON envelope missing data")?;
    if data.get("remote").and_then(Value::as_str) != Some(remote) {
        bail!("open JSON remote mismatch: expected {remote:?}, got {data}");
    }
    if data.get("remote_url").and_then(Value::as_str) != Some(ORIGIN_URL) {
        bail!("open JSON remote_url mismatch: expected {ORIGIN_URL:?}, got {data}");
    }
    if data.get("web_url").and_then(Value::as_str) != Some(web_url) {
        bail!("open JSON web_url mismatch: expected {web_url:?}, got {data}");
    }
    if data.get("target_type").and_then(Value::as_str) != Some(target_type) {
        bail!("open JSON target_type mismatch: expected {target_type:?}, got {data}");
    }
    if data.get("platform").and_then(Value::as_str) != Some(platform) {
        bail!("open JSON platform mismatch: expected {platform:?}, got {data}");
    }
    if data.get("launched").and_then(Value::as_bool) != Some(false) {
        bail!("open JSON launched was not false: {data}");
    }
    Ok(())
}
