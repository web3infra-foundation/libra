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
    fs::write(repo.join("range-a.txt"), "range-a\n").context("write range-a")?;
    ctx.command(&["add", "range-a.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "range a", "--no-verify"],
        repo.clone(),
        true,
    )?;
    fs::write(repo.join("range-b.txt"), "range-b\n").context("write range-b")?;
    ctx.command(&["add", "range-b.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "range b", "--no-verify"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "revert", "HEAD~2..HEAD"], repo.clone(), true)?,
        "revert",
    )?;
    if repo.join("range-a.txt").exists() || repo.join("range-b.txt").exists() {
        bail!("range revert should remove range-a.txt and range-b.txt");
    }
    assert_lbr_or_text(
        &ctx.command(&["revert", "--continue"], repo.clone(), false)?,
        "revert",
    )?;
    assert_lbr_or_text(
        &ctx.command(&["revert", "--abort"], repo.clone(), false)?,
        "revert",
    )?;

    fs::write(repo.join("rename-base.txt"), "line1\nline2\nline3\nline4\n")
        .context("write rename base")?;
    ctx.command(&["add", "rename-base.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "rename base", "--no-verify"],
        repo.clone(),
        true,
    )?;
    fs::write(repo.join("stable-rename.txt"), "stable\n").context("write stable rename")?;
    ctx.command(&["add", "stable-rename.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "stable rename", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["branch", "rename-side"], repo.clone(), true)?;
    ctx.command(&["switch", "rename-side"], repo.clone(), true)?;
    fs::write(
        repo.join("rename-base.txt"),
        "line1\nline2-feature\nline3\nline4\n",
    )
    .context("write rename-side edit")?;
    ctx.command(&["add", "rename-base.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "rename side edit", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["switch", "main"], repo.clone(), true)?;
    fs::write(
        repo.join("renamed.txt"),
        "line1\nline2\nline3\nline4-main\n",
    )
    .context("write renamed file")?;
    ctx.command(&["add", "renamed.txt"], repo.clone(), true)?;
    ctx.command(&["rm", "rename-base.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "rename on main", "--no-verify"],
        repo.clone(),
        true,
    )?;
    let strict_rename = ctx.command(
        &["merge", "--find-renames=90", "rename-side"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&strict_rename, "merge")?;
    ctx.command(&["merge", "--abort"], repo.clone(), true)?;
    ctx.command(
        &["merge", "--find-renames=70", "rename-side"],
        repo.clone(),
        true,
    )?;
    ensure_file(repo.join("renamed.txt"))?;

    let squash_continue = ctx.command(&["merge", "--squash", "--continue"], repo.clone(), false)?;
    assert_lbr_or_text(&squash_continue, "--squash")?;
    let bad_merge = ctx.command(&["merge", "nonexistent-branch"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_merge, "merge")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    assert_criss_cross_rebase_error(ctx)?;
    Ok(())
}

fn assert_criss_cross_rebase_error(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("criss-cross");
    create_committed_repo(ctx, &repo)?;

    ctx.command(&["switch", "-c", "left"], repo.clone(), true)?;
    fs::write(repo.join("left.txt"), "left\n").context("write left side")?;
    ctx.command(&["add", "left.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "left side", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["branch", "left-base"], repo.clone(), true)?;

    ctx.command(&["switch", "main"], repo.clone(), true)?;
    ctx.command(&["switch", "-c", "right"], repo.clone(), true)?;
    fs::write(repo.join("right.txt"), "right\n").context("write right side")?;
    ctx.command(&["add", "right.txt"], repo.clone(), true)?;
    ctx.command(
        &["commit", "-m", "right side", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["branch", "right-base"], repo.clone(), true)?;

    ctx.command(&["switch", "left"], repo.clone(), true)?;
    ctx.command(&["merge", "right-base"], repo.clone(), true)?;
    ctx.command(&["switch", "right"], repo.clone(), true)?;
    ctx.command(&["merge", "left-base"], repo.clone(), true)?;

    ctx.command(&["switch", "left"], repo.clone(), true)?;
    let criss_cross = ctx.command(&["--json", "rebase", "right"], repo.clone(), false)?;
    assert_json_error_code(&criss_cross, "LBR-CONFLICT-002")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
