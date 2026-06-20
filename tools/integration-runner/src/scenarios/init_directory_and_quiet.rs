use super::prelude::*;

pub(crate) fn scenario_init_directory_and_quiet(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    ctx.command(&["init", "nested/repo"], ctx.run_dir.clone(), true)?;
    let nested = ctx.run_dir.join("nested/repo");
    ensure_file(nested.join(".libra/libra.db"))?;
    ensure_file(nested.join(".libra/objects"))?;
    ctx.command(&["status"], nested.clone(), true)?;
    let quiet_short = ctx.command(&["init", "-q", "quiet-short"], ctx.run_dir.clone(), true)?;
    if !String::from_utf8_lossy(&quiet_short.stdout)
        .trim()
        .is_empty()
    {
        bail!("init -q wrote stdout");
    }
    let quiet_long = ctx.command(
        &["init", "--quiet", "quiet-long"],
        ctx.run_dir.clone(),
        true,
    )?;
    if !String::from_utf8_lossy(&quiet_long.stdout)
        .trim()
        .is_empty()
    {
        bail!("init --quiet wrote stdout");
    }
    let quiet_short_repo = ctx.run_dir.join("quiet-short");
    let quiet_long_repo = ctx.run_dir.join("quiet-long");
    ensure_file(quiet_short_repo.join(".libra/libra.db"))?;
    ensure_file(quiet_long_repo.join(".libra/libra.db"))?;
    ctx.command(&["fsck", "--connectivity-only"], quiet_short_repo, true)?;
    let json = ctx.command(
        &["--json", "init", "-q", "quiet-json-repo"],
        ctx.run_dir.clone(),
        true,
    )?;
    assert_json_ok(&json, "init")?;
    ensure_file(ctx.run_dir.join("quiet-json-repo/.libra/libra.db"))?;
    Ok(())
}
