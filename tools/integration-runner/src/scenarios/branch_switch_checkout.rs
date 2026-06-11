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
    ctx.command(&["checkout", "-b", "compat-checkout"], repo.clone(), true)?;
    let compat_current = ctx.command(&["branch", "--show-current"], repo.clone(), true)?;
    assert_stdout_contains(&compat_current, "compat-checkout")?;
    ctx.command(&["checkout", "main"], repo.clone(), true)?;
    ctx.command(
        &["switch", "-C", "reset-feature", "main"],
        repo.clone(),
        true,
    )?;
    let reset_current = ctx.command(&["branch", "--show-current"], repo.clone(), true)?;
    assert_stdout_contains(&reset_current, "reset-feature")?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    ctx.command(&["branch", "force-target", "main"], repo.clone(), true)?;
    ctx.command(&["switch", "force-target"], repo.clone(), true)?;
    fs::write(repo.join("tracked.txt"), "force-target\n").context("write force target")?;
    ctx.command(&["add", "tracked.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "force target", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    fs::write(repo.join("tracked.txt"), "dirty local\n").context("dirty tracked")?;
    ctx.command(&["switch", "-f", "force-target"], repo.clone(), true)?;
    let forced_content =
        fs::read_to_string(repo.join("tracked.txt")).context("read forced tracked")?;
    if forced_content != "force-target\n" {
        bail!("switch -f did not restore target branch content");
    }
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    ctx.command(&["switch", "--orphan", "orphan-root"], repo.clone(), true)?;
    let unborn_head = ctx.command(&["rev-parse", "HEAD"], repo.clone(), false)?;
    assert_lbr_or_text(&unborn_head, "HEAD")?;
    if repo.join("tracked.txt").exists() {
        bail!("switch --orphan should remove previously tracked files");
    }
    ctx.command(&["switch", "main"], repo.clone(), true)?;

    let remote = ctx.repo("guess-remote");
    create_committed_repo(ctx, &remote)?;
    ctx.command(&["switch", "-c", "guessed"], remote.clone(), true)?;
    fs::write(remote.join("guessed.txt"), "guessed\n").context("write guessed file")?;
    ctx.command(&["add", "guessed.txt"], remote.clone(), true)?;
    ctx.command(
        &["commit", "-m", "guessed branch", "--no-verify"],
        remote.clone(),
        true,
    )?;
    ctx.command(&["switch", "-c", "guessed-two"], remote.clone(), true)?;
    let remote_arg = remote.display().to_string();
    ctx.command(
        &["remote", "add", "origin", &remote_arg],
        repo.clone(),
        true,
    )?;
    ctx.command(&["fetch", "origin"], repo.clone(), true)?;
    let remote_branches = ctx.command(&["branch", "-r"], repo.clone(), true)?;
    assert_stdout_contains(&remote_branches, "origin/guessed")?;
    let all_branches = ctx.command(&["branch", "-a"], repo.clone(), true)?;
    assert_stdout_contains(&all_branches, "origin/guessed")?;
    assert_stdout_contains(&all_branches, "main")?;
    ctx.command(&["switch", "guessed"], repo.clone(), true)?;
    let guessed_current = ctx.command(&["branch", "--show-current"], repo.clone(), true)?;
    assert_stdout_contains(&guessed_current, "guessed")?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    let no_guess = ctx.command(&["switch", "--no-guess", "guessed-two"], repo.clone(), false)?;
    assert_lbr_or_text(&no_guess, "not found")?;
    ctx.command(&["switch", "--guess", "guessed-two"], repo.clone(), true)?;
    let guessed_two = ctx.command(&["branch", "--show-current"], repo.clone(), true)?;
    assert_stdout_contains(&guessed_two, "guessed-two")?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;

    let base_commit = stdout_trim(&ctx.command(&["rev-parse", "HEAD"], repo.clone(), true)?);
    ctx.command(&["switch", "--detach", &base_commit], repo.clone(), true)?;
    let detached_sym = ctx.command(&["symbolic-ref", "HEAD"], repo.clone(), false)?;
    assert_lbr_or_text(&detached_sym, "not a symbolic ref")?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;

    ctx.command(
        &["branch", "-m", "feature", "renamed-feature"],
        repo.clone(),
        true,
    )?;
    let branches = ctx.command(&["branch", "--list"], repo.clone(), true)?;
    assert_stdout_contains(&branches, "renamed-feature")?;
    let json_branches = ctx.command(&["--json", "branch", "--list"], repo.clone(), true)?;
    assert_json_ok(&json_branches, "branch --list")?;
    assert_stdout_contains(&json_branches, "branches")?;
    assert_stdout_contains(&json_branches, "renamed-feature")?;
    ctx.command(&["switch", "renamed-feature"], repo.clone(), true)?;
    ctx.command(&["checkout", "--detach", "HEAD"], repo.clone(), true)?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    ctx.command(&["branch", "-D", "renamed-feature"], repo.clone(), true)?;
    ctx.command(&["branch", "-d", "reset-feature"], repo.clone(), true)?;
    let after_safe_delete = ctx.command(&["branch"], repo.clone(), true)?;
    assert_not_contains(&after_safe_delete, "reset-feature")?;
    let bad_delete = ctx.command(&["branch", "-d", "nonexistent"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_delete, "not found")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
