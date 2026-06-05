use super::prelude::*;

pub(crate) fn scenario_config_basic_kv(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("config-repo");
    ctx.command(&["init", "config-repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "set", "user.name", "Libra Config Test"],
        repo.clone(),
        true,
    )?;
    let get = ctx.command(&["config", "get", "user.name"], repo.clone(), true)?;
    assert_stdout_contains(&get, "Libra Config Test")?;
    let list = ctx.command(&["config", "list"], repo.clone(), true)?;
    assert_stdout_contains(&list, "user.name")?;
    ctx.command(&["config", "unset", "user.name"], repo.clone(), true)?;
    let missing = ctx.command(&["config", "get", "user.name"], repo.clone(), false)?;
    assert_lbr_or_text(&missing, "not found")?;
    let fallback = ctx.command(
        &["config", "get", "--default", "fallback", "user.name"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&fallback, "fallback")?;
    ctx.command(&["fsck", "--connectivity-only"], repo.clone(), true)?;
    Ok(())
}
