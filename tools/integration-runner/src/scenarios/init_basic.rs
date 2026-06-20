use super::prelude::*;

pub(crate) fn scenario_init_basic(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    ctx.command(&["init", "repo"], ctx.run_dir.clone(), true)?;
    ensure_file(repo.join(".libra/libra.db"))?;
    ensure_file(repo.join(".libra/objects"))?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    // DIRECTORY positional defaults to "." — plain `libra init` run inside the
    // target directory must initialize that directory in place.
    let default_dir = ctx.repo("default-dir");
    ctx.command(&["init"], default_dir.clone(), true)?;
    ensure_file(default_dir.join(".libra/libra.db"))?;
    let bad = ctx.command(&["status"], ctx.repo("not-a-repo"), false)?;
    assert_lbr_or_text(&bad, "not a libra repository")?;
    Ok(())
}
