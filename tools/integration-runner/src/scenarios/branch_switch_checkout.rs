use super::prelude::*;

pub(crate) fn scenario_branch_switch_checkout(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    ctx.command(&["branch", "feature"], repo.clone(), true)?;
    ctx.command(&["switch", "feature"], repo.clone(), true)?;
    let current = ctx.command(&["branch", "--show-current"], repo.clone(), true)?;
    assert_stdout_contains(&current, "feature")?;
    fs::write(repo.join("feature.txt"), "feature\n").context("write feature file")?;
    ctx.command(&["add", "feature.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "feature work", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    ctx.command(
        &["checkout", "feature", "--", "feature.txt"],
        repo.clone(),
        true,
    )?;
    ensure_file(repo.join("feature.txt"))?;
    ctx.command(&["add", "feature.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "checkout path", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["branch", "-m", "feature", "renamed-feature"],
        repo.clone(),
        true,
    )?;
    let branches = ctx.command(&["branch"], repo.clone(), true)?;
    assert_stdout_contains(&branches, "renamed-feature")?;
    ctx.command(&["switch", "renamed-feature"], repo.clone(), true)?;
    ctx.command(&["checkout", "--detach", "HEAD"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    ctx.command(&["branch", "-D", "renamed-feature"], repo.clone(), true)?;
    let bad_delete = ctx.command(&["branch", "-d", "nonexistent"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_delete, "not found")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
