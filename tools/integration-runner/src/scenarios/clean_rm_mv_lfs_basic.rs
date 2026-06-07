use super::prelude::*;

pub(crate) fn scenario_clean_rm_mv_lfs_basic(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::write(repo.join("temp.tmp"), "temp\n").context("write temp")?;
    assert_stdout_contains(
        &ctx.command(&["clean", "-n"], repo.clone(), true)?,
        "temp.tmp",
    )?;
    ctx.command(&["clean", "-f"], repo.clone(), true)?;
    if repo.join("temp.tmp").exists() {
        bail!("clean -f did not remove temp.tmp");
    }
    fs::write(repo.join("old.txt"), "old\n").context("write old")?;
    ctx.command(&["add", "old.txt"], repo.clone(), true)?;
    ctx.command(&["commit", "-m", "old", "--no-verify"], repo.clone(), true)?;
    ctx.command(&["mv", "old.txt", "new.txt"], repo.clone(), true)?;
    ensure_file(repo.join("new.txt"))?;
    ctx.command(&["add", "new.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "rename", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["rm", "new.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "remove", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["lfs", "track", "*.bin"], repo.clone(), true)?;
    ensure_file(repo.join(".libra_attributes"))?;
    let lfs = ctx.command(&["lfs", "ls-files"], repo.clone(), true)?;
    assert_not_contains(&lfs, "PRIVATE KEY")?;
    ctx.command(&["lfs", "untrack", "*.bin"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    let bad_rm = ctx.command(&["rm", "nonexistent.txt"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_rm, "pathspec")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
