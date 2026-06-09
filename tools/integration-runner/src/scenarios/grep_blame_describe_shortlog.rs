use super::prelude::*;

pub(crate) fn scenario_grep_blame_describe_shortlog(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::write(repo.join("search.txt"), "needle\nsecond\n").context("write search file")?;
    ctx.command(&["add", "search.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "searchable", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["tag", "-m", "inspect release", "v1.0.0"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(&["grep", "needle"], repo.clone(), true)?,
        "search.txt",
    )?;
    assert_stdout_contains(
        &ctx.command(&["blame", "-L", "1,1", "search.txt"], repo.clone(), true)?,
        "needle",
    )?;
    if stdout_trim(&ctx.command(&["describe", "--always"], repo.clone(), true)?).is_empty() {
        bail!("describe --always returned empty output");
    }
    assert_stdout_contains(
        &ctx.command(&["describe", "--tags", "HEAD"], repo.clone(), true)?,
        "v1.0.0",
    )?;
    if stdout_trim(&ctx.command(
        &["describe", "--always", "--abbrev", "12", "HEAD"],
        repo.clone(),
        true,
    )?)
    .is_empty()
    {
        bail!("describe --always --abbrev 12 returned empty output");
    }
    assert_stdout_contains(
        &ctx.command(&["shortlog", "-s", "-n"], repo.clone(), true)?,
        "Libra",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "grep", "needle"], repo.clone(), true)?,
        "grep",
    )?;
    assert_json_ok(
        &ctx.command(
            &["--json", "describe", "--tags", "HEAD"],
            repo.clone(),
            true,
        )?,
        "describe",
    )?;
    assert_lbr_or_text(
        &ctx.command(&["describe", "no-such-revision"], repo.clone(), false)?,
        "invalid",
    )?;
    assert_lbr_or_text(
        &ctx.command(&["grep", "no-such-pattern"], repo, false)?,
        "not found",
    )?;
    Ok(())
}
