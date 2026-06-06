use super::prelude::*;

pub(crate) fn scenario_merge_rebase_cherry_revert_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    ctx.command(&["switch", "-c", "feature"], repo.clone(), true)?;
    fs::write(repo.join("feature.txt"), "feature\n").context("write feature")?;
    ctx.command(&["add", "feature.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "feature", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    fs::write(repo.join("main.txt"), "main\n").context("write main")?;
    ctx.command(&["add", "main.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "main work", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["merge", "feature"], repo.clone(), true)?;
    ensure_file(repo.join("feature.txt"))?;
    ctx.command(&["switch", "-c", "topic"], repo.clone(), true)?;
    fs::write(repo.join("topic.txt"), "topic\n").context("write topic")?;
    ctx.command(&["add", "topic.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "topic", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let topic_commit = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    ctx.command(&["cherry-pick", &topic_commit], repo.clone(), true)?;
    ensure_file(repo.join("topic.txt"))?;
    ctx.command(&["revert", "HEAD"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "log", "--oneline"], repo.clone(), true)?,
        "log",
    )?;
    let bad_merge = ctx.command(&["merge", "nonexistent-branch"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_merge, "merge")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
