use super::prelude::*;

pub(crate) fn scenario_gc_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;

    fs::write(repo.join("unreachable.txt"), "gc unreachable blob\n")
        .context("write unreachable blob fixture")?;
    let hash = ctx.command(
        &["hash-object", "-w", "unreachable.txt"],
        repo.clone(),
        true,
    )?;
    let object_id = stdout_trim(&hash);
    assert_stdout_contains(
        &ctx.command(&["cat-file", "-t", &object_id], repo.clone(), true)?,
        "blob",
    )?;

    let missing_gc = ctx.command(&["--json", "gc", "--dry-run"], repo.clone(), false)?;
    assert_json_error_code(&missing_gc, "LBR-CLI-001")?;
    let missing_prune = ctx.command(&["--json", "prune", "--dry-run"], repo.clone(), false)?;
    assert_json_error_code(&missing_prune, "LBR-CLI-001")?;
    let maintenance = ctx.command(
        &["--json", "maintenance", "run", "--dry-run", "--task", "gc"],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&maintenance, "maintenance run")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
