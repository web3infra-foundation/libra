use super::prelude::*;

pub(crate) fn scenario_config_get_default_and_patterns(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("config-repo");
    ctx.command(&["init", "config-repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "set", "user.name", "Pattern User"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "user.email", "pattern@example.invalid"],
        repo.clone(),
        true,
    )?;
    ctx.command(&["config", "set", "core.editor", "vim"], repo.clone(), true)?;
    ctx.command(
        &[
            "config",
            "set",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &[
            "config",
            "set",
            "--add",
            "remote.origin.fetch",
            "+refs/tags/*:refs/tags/*",
        ],
        repo.clone(),
        true,
    )?;
    let get = ctx.command(&["config", "get", "user.name"], repo.clone(), true)?;
    let compat_get = ctx.command(&["config", "--get", "user.name"], repo.clone(), true)?;
    if stdout_trim(&get) != stdout_trim(&compat_get) {
        bail!("config get and --get differed");
    }
    let default = ctx.command(
        &["config", "get", "--default", "fallback", "missing.key"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&default, "fallback")?;
    let default_short = ctx.command(
        &["config", "get", "-d", "fallback-short", "missing.short"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&default_short, "fallback-short")?;
    let regexp = ctx.command(
        &["config", "get", "--regexp", "^user\\."],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&regexp, "user.name")?;
    assert_stdout_contains(&regexp, "user.email")?;
    let compat_regexp = ctx.command(&["config", "--get-regexp", "^user\\."], repo.clone(), true)?;
    assert_stdout_contains(&compat_regexp, "user.name")?;
    let get_all = ctx.command(
        &["config", "--get-all", "remote.origin.fetch"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&get_all, "+refs/heads/*:refs/remotes/origin/*")?;
    assert_stdout_contains(&get_all, "+refs/tags/*:refs/tags/*")?;
    let json_default = ctx.command(
        &[
            "--json",
            "config",
            "get",
            "--default",
            "fallback",
            "missing.key",
        ],
        repo.clone(),
        true,
    )?;
    assert_json_ok(&json_default, "config")?;
    Ok(())
}
