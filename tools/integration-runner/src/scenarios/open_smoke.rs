use super::prelude::*;

pub(crate) fn scenario_open_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    let origin = "https://example.invalid/owner/repo.git";
    ctx.command(&["remote", "add", "origin", origin], repo.clone(), true)?;
    let output = ctx.command(&["--json", "open"], repo.clone(), true)?;
    assert_json_ok(&output, "open")?;
    let output = ctx.command(&["--json", "open", "origin"], repo.clone(), true)?;
    assert_json_ok(&output, "open")?;
    let bad_open = ctx.command(&["open", "nonexistent-remote"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_open, "invalid")?;
    Ok(())
}
