use super::prelude::*;

pub(crate) fn scenario_notes_smoke(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    let missing = ctx.command(
        &["--json", "notes", "add", "-m", "first note"],
        repo.clone(),
        false,
    )?;
    assert_json_error_code(&missing, "LBR-CLI-001")?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
