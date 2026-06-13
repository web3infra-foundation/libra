use super::prelude::*;

pub(crate) fn scenario_tag_basic(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("repo");
    create_committed_repo(ctx, &repo)?;
    ctx.command(&["tag", "v1.0.0"], repo.clone(), true)?;
    assert_stdout_contains(&ctx.command(&["tag", "-l"], repo.clone(), true)?, "v1.0.0")?;
    ctx.command(
        &["tag", "-m", "release v1.1.0", "v1.1.0"],
        repo.clone(),
        true,
    )?;
    let listed = ctx.command(&["tag", "-l", "-n", "1"], repo.clone(), true)?;
    assert_stdout_contains(&listed, "release v1.1.0")?;
    let file_message = ctx.command(&["tag", "-F", "release.txt", "v1.2.0"], repo.clone(), false)?;
    assert_lbr_or_text(&file_message, "-F")?;
    let rev = ctx.command(&["rev-parse", "v1.0.0"], repo.clone(), true)?;
    if stdout_trim(&rev).len() < 40 {
        bail!("tag rev-parse returned short id");
    }
    assert_stdout_contains(
        &ctx.command(&["describe", "--tags", "--always"], repo.clone(), true)?,
        "v1",
    )?;
    ctx.command(&["tag", "-f", "v1.0.0"], repo.clone(), true)?;
    ctx.command(&["tag", "-d", "v1.1.0"], repo.clone(), true)?;
    assert_lbr_or_text(
        &ctx.command(&["rev-parse", "v1.1.0"], repo.clone(), false)?,
        "not found",
    )?;
    assert_lbr_or_text(
        &ctx.command(&["tag", "v1.3.0", "v1.4.0"], repo.clone(), false)?,
        "only --delete accepts multiple tag names",
    )?;
    assert_json_ok(
        &ctx.command(&["--json", "tag", "-d", "v1.0.0"], repo.clone(), true)?,
        "tag",
    )?;
    let missing_json_delete =
        ctx.command(&["--json", "tag", "-d", "missing-tag"], repo.clone(), false)?;
    assert_json_error_code(&missing_json_delete, "LBR-CLI-003")?;
    assert_json_ok(
        &ctx.command(&["--json", "tag", "-l"], repo.clone(), true)?,
        "tag",
    )?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    Ok(())
}
