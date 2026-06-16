use super::prelude::*;

pub(crate) fn assert_rev_list_readback(
    ctx: &mut ScenarioCtx<'_>,
    repo: &Path,
    head_id: &str,
) -> Result<()> {
    fs::write(repo.join("docs/rev-list.md"), "rev-list second\n")
        .context("write rev-list second fixture")?;
    ctx.command(&["add", "docs/rev-list.md"], repo.to_path_buf(), true)?;
    ctx.command(
        &["commit", "-m", "test: rev-list second", "--no-verify"],
        repo.to_path_buf(),
        true,
    )?;
    let latest_id = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.to_path_buf(), true)?);
    let rev_list = ctx.command(&["rev-list", "HEAD"], repo.to_path_buf(), true)?;
    assert_stdout_contains(&rev_list, head_id)?;

    let rev_count = ctx.command(&["rev-list", "--count", "HEAD"], repo.to_path_buf(), true)?;
    if stdout_trim(&rev_count) != "2" {
        bail!("rev-list --count HEAD returned unexpected count");
    }

    let rev_limit = ctx.command(&["rev-list", "-n", "1", "HEAD"], repo.to_path_buf(), true)?;
    let rev_limit_stdout = String::from_utf8_lossy(&rev_limit.stdout);
    if rev_limit_stdout.lines().count() != 1 {
        bail!("rev-list -n 1 HEAD returned more than one commit");
    }

    let rev_skip = ctx.command(
        &["rev-list", "--skip", "1", "--max-count", "1", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_skip) != head_id {
        bail!("rev-list --skip 1 --max-count 1 HEAD did not return the parent commit");
    }

    let rev_min_parents = ctx.command(
        &["rev-list", "--min-parents", "1", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_min_parents) != latest_id {
        bail!("rev-list --min-parents 1 HEAD did not return the non-root commit");
    }

    let rev_max_parents = ctx.command(
        &["rev-list", "--max-parents", "0", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_max_parents) != head_id {
        bail!("rev-list --max-parents 0 HEAD did not return the root commit");
    }

    let rev_no_merges = ctx.command(
        &["rev-list", "--no-merges", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    let rev_no_merges_output = String::from_utf8_lossy(&rev_no_merges.stdout);
    if rev_no_merges_output.lines().collect::<Vec<_>>() != vec![latest_id.as_str(), head_id] {
        bail!("rev-list --no-merges HEAD did not keep linear commits in traversal order");
    }

    let rev_merges = ctx.command(&["rev-list", "--merges", "HEAD"], repo.to_path_buf(), true)?;
    if !stdout_trim(&rev_merges).is_empty() {
        bail!("rev-list --merges HEAD returned commits for a linear history");
    }

    let rev_merge_count = ctx.command(
        &["rev-list", "--count", "--merges", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_merge_count) != "0" {
        bail!("rev-list --count --merges HEAD returned unexpected count");
    }

    let rev_parents = ctx.command(&["rev-list", "--parents", "HEAD"], repo.to_path_buf(), true)?;
    let rev_parents_output = String::from_utf8_lossy(&rev_parents.stdout);
    let Some(first_parent_line) = rev_parents_output.lines().next() else {
        bail!("rev-list --parents HEAD returned empty output");
    };
    if !first_parent_line.starts_with(&latest_id) || !first_parent_line.ends_with(head_id) {
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
