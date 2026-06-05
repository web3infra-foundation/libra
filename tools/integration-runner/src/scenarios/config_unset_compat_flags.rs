use super::prelude::*;

pub(crate) fn scenario_config_unset_compat_flags(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("config-repo");
    ctx.command(&["init", "config-repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "set", "temp.single", "value"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["config", "--unset", "temp.single"], repo.clone(), true)?;
    let missing = ctx.command(&["config", "get", "temp.single"], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "not found")?;
    ctx.command(
        &["config", "set", "--add", "temp.multi", "one"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "--add", "temp.multi", "two"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "unset", "--all", "temp.multi"],
        repo.clone(),
        true,
    )?;
    let missing_multi = ctx.command(
        &["config", "get", "--all", "temp.multi"],
        repo.clone(),
        true,
    )?;
    if !stdout_trim(&missing_multi).is_empty() {
        bail!("config get --all temp.multi returned values after unset --all");
    }
    ctx.command(
        &["config", "set", "--add", "temp.legacy", "one"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "--add", "temp.legacy", "two"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "--unset-all", "temp.legacy"],
        repo.clone(),
        true,
    )?;
    let missing_legacy =
        ctx.command(&["config", "--get-all", "temp.legacy"], repo.clone(), true)?;
    if !stdout_trim(&missing_legacy).is_empty() {
        bail!("config --get-all temp.legacy returned values after --unset-all");
    }
    Ok(())
}
