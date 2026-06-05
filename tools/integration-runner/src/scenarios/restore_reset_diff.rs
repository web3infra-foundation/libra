use super::prelude::*;

pub(crate) fn scenario_restore_reset_diff(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::write(repo.join("tracked.txt"), "modified\n").context("modify tracked file")?;
    assert_stdout_contains(&ctx.command(&["diff"], repo.clone(), true)?, "modified")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--staged"], repo.clone(), true)?,
        "modified",
    )?;
    ctx.command(&["restore", "--staged", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    let restored = fs::read_to_string(repo.join("tracked.txt")).context("read restored file")?;
    if restored != "base\n" {
        bail!("restore did not return tracked.txt to base content: {restored:?}");
    }
    fs::write(repo.join("tracked.txt"), "second\n").context("modify tracked second")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "second", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["reset", "--soft", "HEAD~1"], repo.clone(), true)?;
    ctx.command(&["reset", "--mixed", "HEAD"], repo.clone(), true)?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
