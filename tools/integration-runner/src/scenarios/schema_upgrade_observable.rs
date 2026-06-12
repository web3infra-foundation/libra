use std::process::Output;

use serde_json::Value;

use super::prelude::*;

pub(crate) fn scenario_schema_upgrade_observable(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;

    let status_json = ctx.command(&["--json", "db", "status"], repo.clone(), true)?;
    assert_db_status_compatible(&status_json)?;
    ctx.command(&["db", "status"], repo.clone(), true)?;

    // Idempotency: `db upgrade` on an up-to-date repo succeeds repeatedly
    // with no migrations applied (doc supplementary assertion).
    ctx.command(&["db", "upgrade"], repo.clone(), true)?;
    ctx.command(&["db", "upgrade"], repo.clone(), true)?;
    let upgrade_json = ctx.command(&["--json", "db", "upgrade"], repo.clone(), true)?;
    assert_db_upgrade_noop(&upgrade_json)?;
    ctx.command(&["db", "status"], repo.clone(), true)?;

    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;

    let not_a_repo = ctx.repo("not-a-repo");
    let bad_status = ctx.command(&["db", "status"], not_a_repo.clone(), false)?;
    assert_lbr_or_text(&bad_status, "not a libra repository")?;
    let bad_upgrade = ctx.command(&["db", "upgrade"], not_a_repo, false)?;
    assert_lbr_or_text(&bad_upgrade, "not a libra repository")?;
    Ok(())
}

fn db_json_data(output: &Output, command: &str) -> Result<Value> {
    assert_json_ok(output, command)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("parse {command} JSON: {stdout}"))?;
    value
        .get("data")
        .cloned()
        .with_context(|| format!("{command} JSON envelope missing data: {value}"))
}

fn assert_db_status_compatible(output: &Output) -> Result<()> {
    let data = db_json_data(output, "db status")?;
    if data.get("state").and_then(Value::as_str) != Some("compatible") {
        bail!("db status state was not compatible: {data}");
    }
    let current = data.get("current_version").and_then(Value::as_i64);
    let latest = data.get("latest_version").and_then(Value::as_i64);
    if current.is_none() || current != latest {
        bail!("db status current/latest mismatch: {data}");
    }
    Ok(())
}

fn assert_db_upgrade_noop(output: &Output) -> Result<()> {
    let data = db_json_data(output, "db upgrade")?;
    if data.get("upgraded").and_then(Value::as_bool) != Some(false) {
        bail!("db upgrade on up-to-date repo reported upgraded=true: {data}");
    }
    let applied = data
        .get("applied_versions")
        .and_then(Value::as_array)
        .with_context(|| format!("db upgrade JSON missing applied_versions: {data}"))?;
    if !applied.is_empty() {
        bail!("db upgrade on up-to-date repo applied migrations: {data}");
    }
    let current = data.get("current_version").and_then(Value::as_i64);
    let latest = data.get("latest_version").and_then(Value::as_i64);
    if current.is_none() || current != latest {
        bail!("db upgrade current/latest mismatch: {data}");
    }
    Ok(())
}
