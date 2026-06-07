use super::prelude::*;

pub(crate) fn scenario_schema_upgrade_observable(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    assert_json_ok(
        &ctx.command(&["--json", "db", "status"], repo.clone(), true)?,
        "db",
    )?;
    ctx.command(&["db", "status"], repo.clone(), true)?;
    ctx.command(&["db", "upgrade"], repo.clone(), true)?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    let bad = ctx.command(&["db", "status"], ctx.repo("not-a-repo"), false)?;
    assert_lbr_or_text(&bad, "not a libra repository")?;
    Ok(())
}
