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
        &["config", "user.name", "Rev List Committer"],
        repo.to_path_buf(),
        true,
    )?;
    ctx.command(
        &[
            "config",
            "user.email",
            "rev-list-committer@example.com",
        ],
        repo.to_path_buf(),
        true,
    )?;
    ctx.command(
        &[
            "commit",
            "-m",
            "test: rev-list second",
            "--author",
            "Rev List Author <rev-list@example.com>",
            "--no-verify",
        ],
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

    let rev_multi = ctx.command(
        &["rev-list", "HEAD", "HEAD~1"],
        repo.to_path_buf(),
        true,
    )?;
    let rev_multi_output = String::from_utf8_lossy(&rev_multi.stdout);
    if rev_multi_output.lines().collect::<Vec<_>>() != vec![latest_id.as_str(), head_id] {
        bail!("rev-list HEAD HEAD~1 did not de-duplicate multi revision output");
    }

    let rev_range = ctx.command(
        &["rev-list", "HEAD~1..HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_range) != latest_id {
        bail!("rev-list HEAD~1..HEAD did not return only the tip commit");
    }

    let rev_exclude = ctx.command(
        &["rev-list", "^HEAD~1", "HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_exclude) != latest_id {
        bail!("rev-list ^HEAD~1 HEAD did not exclude the parent history");
    }

    let rev_symmetric = ctx.command(
        &["rev-list", "HEAD~1...HEAD"],
        repo.to_path_buf(),
        true,
    )?;
    if stdout_trim(&rev_symmetric) != latest_id {
        bail!("rev-list HEAD~1...HEAD did not return the symmetric difference");
    }

    super::object_readback_rev_list_filters::assert_rev_list_filters(
        ctx, repo, head_id, &latest_id,
    )?;
    super::object_readback_rev_list_output::assert_rev_list_output(
        ctx, repo, head_id, &latest_id,
    )?;

    Ok(())
}
