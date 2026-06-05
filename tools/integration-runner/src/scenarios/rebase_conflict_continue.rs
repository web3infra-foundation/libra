use super::prelude::*;

pub(crate) fn scenario_rebase_conflict_continue(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    ctx.command(&["switch", "-c", "topic"], repo.clone(), true)?;
    fs::write(repo.join("tracked.txt"), "topic\n").context("write topic conflict")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "topic conflict", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    fs::write(repo.join("tracked.txt"), "main\n").context("write main conflict")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "main conflict", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["switch", "topic"], repo.clone(), true)?;
    let conflict = ctx.command(&["rebase", "main"], repo.clone(), false)?;
    assert_lbr_or_text(&conflict, "conflict")?;
    let content =
        fs::read_to_string(repo.join("tracked.txt")).context("read rebase conflict file")?;
    if !content.contains("<<<<<<<") {
        bail!("rebase conflict did not write conflict markers: {content}");
    }
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    fs::write(repo.join("tracked.txt"), "resolved\n").context("resolve rebase")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["rebase", "--continue"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    let no_session = ctx.command(&["rebase", "--continue"], repo.clone(), false)?;
    assert_lbr_or_text(&no_session, "no rebase")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
