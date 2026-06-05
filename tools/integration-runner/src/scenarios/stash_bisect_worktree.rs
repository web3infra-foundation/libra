use super::prelude::*;

pub(crate) fn scenario_stash_bisect_worktree(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    fs::write(repo.join("tracked.txt"), "stashed\n").context("write stashed change")?;
    ctx.command(&["stash", "push", "-m", "save work"], repo.clone(), true)?;
    assert_stdout_contains(
        &ctx.command(&["stash", "list"], repo.clone(), true)?,
        "save work",
    )?;
    ctx.command(&["stash", "apply"], repo.clone(), true)?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "apply stash", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["stash", "clear", "--force"], repo.clone(), true)?;

    ctx.command(&["bisect", "start"], repo.clone(), true)?;
    ctx.command(&["bisect", "bad"], repo.clone(), true)?;
    ctx.command(&["bisect", "good", "HEAD~1"], repo.clone(), true)?;
    ctx.command(&["bisect", "log"], repo.clone(), true)?;
    ctx.command(&["bisect", "reset"], repo.clone(), true)?;

    let wt = ctx.run_dir.join("wt").to_string_lossy().to_string();
    ctx.command(&["worktree", "add", &wt], repo.clone(), true)?;
    let list = ctx.command(&["worktree", "list"], repo.clone(), true)?;
    assert_stdout_contains(&list, &wt)?;
    ctx.command(&["worktree", "lock", &wt], repo.clone(), true)?;
    ctx.command(&["worktree", "unlock", &wt], repo.clone(), true)?;
    ctx.command(&["worktree", "remove", &wt], repo.clone(), true)?;
    if !Path::new(&wt).exists() {
        bail!("worktree remove unexpectedly deleted directory by default");
    }
    ctx.command(&["worktree", "prune"], repo.clone(), true)?;
    Ok(())
}
