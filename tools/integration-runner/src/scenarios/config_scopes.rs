use super::prelude::*;

pub(crate) fn scenario_config_scopes(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let scope_a = ctx.repo("scope-a");
    let scope_b = ctx.repo("scope-b");
    ctx.command(&["init", "scope-a"], ctx.run_dir.clone(), true)?;
    ctx.command(&["init", "scope-b"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "--local", "set", "test.scope", "local-a"],
        scope_a.clone(),
        true,
    )?;
    ctx.command(
        &["config", "--global", "set", "test.scope", "global-value"],
        scope_a.clone(),
        true,
    )?;
    let local = ctx.command(
        &["config", "--local", "get", "test.scope"],
        scope_a.clone(),
        true,
    )?;
    assert_stdout_contains(&local, "local-a")?;
    let global_a = ctx.command(
        &["config", "--global", "get", "test.scope"],
        scope_a.clone(),
        true,
    )?;
    assert_stdout_contains(&global_a, "global-value")?;
    let global_b = ctx.command(
        &["config", "--global", "get", "test.scope"],
        scope_b.clone(),
        true,
    )?;
    assert_stdout_contains(&global_b, "global-value")?;
    let missing_local = ctx.command(
        &["config", "--local", "get", "test.scope"],
        scope_b.clone(),
        false,
    )?;
    assert_lbr_or_text(&missing_local, "not found")?;
    let system = ctx.command(&["config", "--system", "list"], scope_b.clone(), false)?;
    assert_lbr_or_text(&system, "system")?;
    Ok(())
}
