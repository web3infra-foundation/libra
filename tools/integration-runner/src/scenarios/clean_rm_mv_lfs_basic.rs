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
    fs::write(repo.join("dry.txt"), "dry\n").context("write dry")?;
    fs::write(repo.join("verbose.txt"), "verbose\n").context("write verbose")?;
    fs::write(repo.join("json.txt"), "json\n").context("write json")?;
    ctx.command(
        &["add", "old.txt", "dry.txt", "verbose.txt", "json.txt"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["commit", "-m", "mv fixtures", "--no-verify"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["mv", "old.txt", "new.txt"], repo.clone(), true)?;
    ensure_file(repo.join("new.txt"))?;
    let dry_run = ctx.command(&["mv", "-n", "dry.txt", "dry-new.txt"], repo.clone(), true)?;
    assert_stdout_contains(&dry_run, "Checking rename of 'dry.txt' to 'dry-new.txt'")?;
    assert_stdout_contains(&dry_run, "Renaming dry.txt to dry-new.txt")?;
    ensure_file(repo.join("dry.txt"))?;
    if repo.join("dry-new.txt").exists() {
        bail!("mv -n unexpectedly materialized dry-new.txt");
    }
    let verbose = ctx.command(
        &["mv", "-v", "verbose.txt", "verbose-new.txt"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&verbose, "Renaming verbose.txt to verbose-new.txt")?;
    assert_not_contains(&verbose, "Checking rename of")?;
    ensure_file(repo.join("verbose-new.txt"))?;
    let json_mv = ctx.command(
        &["--json", "mv", "--sparse", "json.txt", "json-new.txt"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&json_mv, "mv")?;
    assert_stdout_contains(&json_mv, "json-new.txt")?;
    assert_not_contains(&json_mv, "\"sparse\"")?;
    ensure_file(repo.join("json-new.txt"))?;
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
    let bad_mv = ctx.command(
        &["mv", "no-such-source.txt", "docs/dest.txt"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_mv, "bad source")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
