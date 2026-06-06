use super::prelude::*;

pub(crate) fn scenario_reflog_symbolic_ref(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    let head = ctx.command(&["symbolic-ref", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&head, "refs/heads/main")?;
    ctx.command(&["branch", "other"], repo.clone(), true)?;
    ctx.command(
        &["symbolic-ref", "HEAD", "refs/heads/other"],
        repo.clone(),
        true,
    )?;
    let head = ctx.command(&["symbolic-ref", "HEAD"], repo.clone(), true)?;
    assert_stdout_contains(&head, "refs/heads/other")?;
    let reflog = ctx.command(&["reflog", "show"], repo.clone(), true)?;
    assert_not_contains(&reflog, "PRIVATE KEY")?;
    ctx.command(&["reflog", "exists", "HEAD"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "show-ref", "--heads"], repo.clone(), true)?,
        "show-ref",
    )?;
    let bad = ctx.command(
        &["symbolic-ref", "refs/custom", "refs/heads/main"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad, "HEAD")?;
    Ok(())
}
