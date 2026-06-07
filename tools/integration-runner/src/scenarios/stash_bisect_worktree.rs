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
    assert_json_ok(
        &ctx.command(&["--json", "stash", "list"], repo.clone(), true)?,
        "stash",
    )?;
    ctx.command(&["stash", "apply"], repo.clone(), true)?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "apply stash", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["stash", "clear", "--force"], repo.clone(), true)?;
    let empty_pop = ctx.command(&["stash", "pop"], repo.clone(), false)?;
    assert_lbr_or_text(&empty_pop, "stash")?;

    fs::write(repo.join("visible-untracked.txt"), "visible\n")
        .context("write visible untracked stash fixture")?;
    let include_untracked = ctx.command(&["--json", "stash", "push", "-u"], repo.clone(), true)?;
    assert_json_ok(&include_untracked, "stash")?;
    assert_stdout_contains(&include_untracked, "included_untracked")?;
    if repo.join("visible-untracked.txt").exists() {
        bail!("stash push -u did not remove included untracked file");
    }
    ctx.command(&["stash", "pop"], repo.clone(), true)?;
    ensure_file(repo.join("visible-untracked.txt"))?;
    fs::remove_file(repo.join("visible-untracked.txt"))
        .context("remove restored visible untracked fixture")?;

    fs::write(repo.join(".libraignore"), "ignored.log\n").context("write stash ignore fixture")?;
    fs::write(repo.join("ignored.log"), "ignored\n").context("write ignored stash fixture")?;
    let include_all = ctx.command(&["--json", "stash", "push", "--all"], repo.clone(), true)?;
    assert_json_ok(&include_all, "stash")?;
    assert_stdout_contains(&include_all, "included_untracked")?;
    if repo.join(".libraignore").exists() || repo.join("ignored.log").exists() {
        bail!("stash push --all did not remove visible and ignored untracked files");
    }
    ctx.command(&["stash", "pop"], repo.clone(), true)?;
    ensure_file(repo.join(".libraignore"))?;
    ensure_file(repo.join("ignored.log"))?;
    fs::remove_file(repo.join(".libraignore")).context("remove restored ignore fixture")?;
    fs::remove_file(repo.join("ignored.log")).context("remove restored ignored fixture")?;

    fs::write(repo.join("tracked.txt"), "staged\n").context("write staged stash fixture")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    fs::write(repo.join("tracked.txt"), "unstaged\n").context("write unstaged stash fixture")?;
    let keep_index = ctx.command(&["--json", "stash", "push", "--keep-index"], repo.clone(), true)?;
    assert_json_ok(&keep_index, "stash")?;
    assert_stdout_contains(&keep_index, "kept_index")?;
    let kept_index =
        fs::read_to_string(repo.join("tracked.txt")).context("read keep-index fixture")?;
    if kept_index != "staged\n" {
        bail!("stash push --keep-index did not keep staged content in worktree: {kept_index}");
    }
    ctx.command(&["reset", "--hard"], repo.clone(), true)?;
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
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    Ok(())
}
