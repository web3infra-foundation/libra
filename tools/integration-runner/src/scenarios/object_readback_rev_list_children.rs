use super::prelude::*;

pub(crate) fn assert_rev_list_children(
    ctx: &mut ScenarioCtx<'_>,
    repo: &Path,
    head_id: &str,
    latest_id: &str,
) -> Result<()> {
    let rev_children = ctx.command(
        &["rev-list", "--children", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    let rev_children_output = String::from_utf8_lossy(&rev_children.stdout);
    if rev_children_output.lines().collect::<Vec<_>>()
        != vec![latest_id.to_string(), format!("{head_id} {latest_id}")]
    {
        bail!("rev-list --children HEAD did not include child commit IDs");
    }

    let rev_children_count = ctx.command(
        &["rev-list", "--count", "--children", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_children_count) != "2" {
        bail!("rev-list --count --children HEAD returned unexpected count");
    }

    let rev_children_json = ctx.command(
        &[
            "--json",
            "rev-list",
            "--children",
            "--skip",
            "1",
            "--max-count",
            "1",
            "HEAD",
        ],
        repo.to_path_buf(),
        true,
    )?;
    let rev_children_json: serde_json::Value = serde_json::from_slice(&rev_children_json.stdout)
        .context("parse rev-list children JSON output")?;
    if rev_children_json["data"]["children"] != serde_json::json!(true) {
        bail!("rev-list --children JSON did not echo flag: {rev_children_json}");
    }
    if rev_children_json["data"]["commits"] != serde_json::json!([head_id]) {
        bail!("rev-list --children JSON did not keep plain commit IDs: {rev_children_json}");
    }
    if rev_children_json["data"]["entries"][0]["children"] != serde_json::json!([latest_id]) {
        bail!("rev-list --children JSON did not include child metadata: {rev_children_json}");
    }

    let conflict = ctx.command(
        &["rev-list", "--parents", "--children", "HEAD"],
        repo.to_path_buf(),
        false,
    )?;
    assert_lbr_or_text(&conflict, "cannot be used with")?;

    Ok(())
}
