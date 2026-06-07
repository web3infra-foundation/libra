use super::prelude::*;

pub(crate) fn scenario_restore_reset_diff(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::write(repo.join("tracked.txt"), "modified\n").context("modify tracked file")?;
    assert_stdout_contains(&ctx.command(&["diff"], repo.clone(), true)?, "modified")?;
    assert_stdout_contains(&ctx.command(&["diff", "tracked.txt"], repo.clone(), true)?, "modified")?;
    assert_stdout_contains(&ctx.command(&["diff", "--name-only"], repo.clone(), true)?, "tracked.txt")?;
    assert_stdout_contains(&ctx.command(&["diff", "--stat"], repo.clone(), true)?, "tracked.txt")?;
    assert_stdout_contains(&ctx.command(&["diff", "--raw"], repo.clone(), true)?, "M\ttracked.txt")?;
    assert_stdout_contains(&ctx.command(&["diff", "-w", "-U0"], repo.clone(), true)?, "modified")?;
    let exit_diff = ctx.command(&["diff", "--exit-code"], repo.clone(), false)?;
    if exit_diff.status.code() != Some(1) {
        bail!(
            "diff --exit-code should exit 1 when changes exist, got {:?}",
            exit_diff.status.code()
        );
    }
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--staged"], repo.clone(), true)?,
        "modified",
    )?;
    assert_stdout_contains(
        &ctx.command(&["diff", "--staged", "--name-status"], repo.clone(), true)?,
        "M\ttracked.txt",
    )?;
    ctx.command(&["restore", "--staged", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["restore", "--worktree", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    let restored = fs::read_to_string(repo.join("tracked.txt")).context("read restored file")?;
    if restored != "base\n" {
        bail!("restore did not return tracked.txt to base content: {restored:?}");
    }
    fs::write(repo.join("tracked.txt"), "second\n").context("modify tracked second")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["reset", "HEAD", "--", "tracked.txt"], repo.clone(), true)?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "second", "--no-verify"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(
        &ctx.command(
            &["diff", "--old", "HEAD~1", "--new", "HEAD", "--numstat"],
            repo.clone(),
            true,
        )?,
        "tracked.txt",
    )?;
    ctx.command(&["reset", "--soft", "HEAD~1"], repo.clone(), true)?;
    ctx.command(&["reset", "--mixed", "HEAD"], repo.clone(), true)?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    fs::write(repo.join("orig.txt"), "l1\nl2\nl3\nl4\n").context("write rename source")?;
    ctx.command(&["add", "orig.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "rename source", "--no-verify"],
        repo.clone(),
        true,
    )?;
    fs::remove_file(repo.join("orig.txt")).context("remove rename source")?;
    fs::write(repo.join("new.txt"), "l1\nl2\nl3\nCHANGED\n").context("write rename dest")?;
    assert_stdout_contains(
        &ctx.command(&["diff", "-M70", "--name-status"], repo.clone(), true)?,
        "R075\torig.txt\tnew.txt",
    )?;
    ctx.command(&["reset", "--hard", "HEAD"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "diff"], repo.clone(), true)?,
        "diff",
    )?;
    let bad_diff = ctx.command(&["diff", "--old", "no-such-revision", "--new", "HEAD"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_diff, "invalid revision")?;
    let bad_restore = ctx.command(&["restore", "nonexistent.txt"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_restore, "pathspec")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
