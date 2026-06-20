use super::prelude::*;

pub(crate) fn assert_rev_list_output(
    ctx: &mut ScenarioCtx<'_>,
    repo: &Path,
    head_id: &str,
    latest_id: &str,
) -> Result<()> {
    let rev_parents = ctx.command(&["rev-list", "--parents", "HEAD"], repo.to_path_buf(), true)?;
    let rev_parents_output = String::from_utf8_lossy(&rev_parents.stdout);
    let Some(first_parent_line) = rev_parents_output.lines().next() else {
        bail!("rev-list --parents HEAD returned empty output");
    };
    if !first_parent_line.starts_with(latest_id) || !first_parent_line.ends_with(head_id) {
        bail!("rev-list --parents HEAD did not include the current HEAD followed by its parent");
    }

    let rev_timestamp = ctx.command(
        &["rev-list", "--timestamp", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    let rev_timestamp_output = String::from_utf8_lossy(&rev_timestamp.stdout);
    let Some(first_timestamp_line) = rev_timestamp_output.lines().next() else {
        bail!("rev-list --timestamp HEAD returned empty output");
    };
    let timestamp_fields = first_timestamp_line.split_whitespace().collect::<Vec<_>>();
    if timestamp_fields.len() != 2
        || timestamp_fields[0].parse::<u64>().is_err()
        || timestamp_fields[1] != latest_id
    {
        bail!("rev-list --timestamp HEAD did not use Git-compatible `timestamp commit` output");
    }

    let rev_timestamp_parents = ctx.command(
        &["rev-list", "--timestamp", "--parents", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    let rev_timestamp_parents_output = String::from_utf8_lossy(&rev_timestamp_parents.stdout);
    let Some(first_timestamp_parent_line) = rev_timestamp_parents_output.lines().next() else {
        bail!("rev-list --timestamp --parents HEAD returned empty output");
    };
    let timestamp_parent_fields = first_timestamp_parent_line
        .split_whitespace()
        .collect::<Vec<_>>();
    if timestamp_parent_fields.len() != 3
        || timestamp_parent_fields[0].parse::<u64>().is_err()
        || timestamp_parent_fields[1] != latest_id
        || timestamp_parent_fields[2] != head_id
    {
        bail!(
            "rev-list --timestamp --parents HEAD did not use Git-compatible `timestamp commit parent` output"
        );
    }

    assert_json_ok(
        &ctx.command(&["--json", "rev-list", "HEAD"], repo.to_path_buf(), true)?,
        "rev-list",
    )?;

    Ok(())
}
