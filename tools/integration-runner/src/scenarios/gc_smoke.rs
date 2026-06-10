use super::prelude::*;

pub(crate) fn scenario_gc_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;

    fs::write(repo.join("unreachable.txt"), "gc unreachable blob\n")
        .context("write unreachable blob fixture")?;
    let hash = ctx.command(&["hash-object", "-w", "unreachable.txt"], repo.clone(), true)?;
    let object_id = stdout_trim(&hash);
    if object_id.len() < 40 {
        bail!("hash-object returned an unexpectedly short id: {object_id}");
    }

    let object_type = ctx.command(&["cat-file", "-t", &object_id], repo.clone(), true)?;
    assert_stdout_contains(&object_type, "blob")?;

    assert_json_ok(
        &ctx.command(
            &["--json", "gc", "--dry-run", "--prune=now"],
            repo.clone(),
            true,
        )?,
        "gc",
    )?;
    let still_present = ctx.command(&["cat-file", "-t", &object_id], repo.clone(), true)?;
    assert_stdout_contains(&still_present, "blob")?;

    ctx.command(&["gc", "--prune=now"], repo.clone(), true)?;
    let missing = ctx.command(&["cat-file", "-t", &object_id], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "object not found")?;

    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
