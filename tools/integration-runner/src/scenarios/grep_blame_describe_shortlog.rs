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
        &ctx.command(&["shortlog", "-s", "-n"], repo.clone(), true)?,
        "Libra",
    )?;
    assert_lbr_or_text(
        &ctx.command(&["grep", "no-such-pattern"], repo, false)?,
        "not found",
    )?;
    Ok(())
}
