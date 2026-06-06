use super::prelude::*;

pub(crate) fn scenario_cross_cutting_flags(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    assert_json_ok(
        &ctx.command(&["--json", "status"], repo.clone(), true)?,
        "status",
    )?;
    let quiet = ctx.command(&["--quiet", "status"], repo.clone(), true)?;
    if !String::from_utf8_lossy(&quiet.stdout).trim().is_empty() {
        bail!("--quiet status wrote stdout");
    }
    let machine = ctx.command(&["--machine", "status"], repo.clone(), true)?;
    assert_not_contains(&machine, "\u{1b}[")?;
    ctx.command(&["--color", "never", "status"], repo.clone(), true)?;
    ctx.command(&["--progress", "none", "status"], repo.clone(), true)?;
    ctx.command(&["--exit-code-on-warning", "status"], repo.clone(), true)?;
    let bad = ctx.command(&["--json", "status"], ctx.repo("not-a-repo"), false)?;
    assert_json_error_code(&bad, "LBR-REPO-001")
        .or_else(|_| assert_lbr_or_text(&bad, "not a libra repository"))?;
    Ok(())
}
