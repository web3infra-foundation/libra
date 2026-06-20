use super::prelude::*;

pub(crate) fn scenario_config_list_variants(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("config-repo");
    ctx.command(&["init", "config-repo"], ctx.run_dir.clone(), true)?;
    ctx.command(
        &["config", "set", "user.name", "List User"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "user.email", "list@example.invalid"],
        repo.clone(),
        true,
    )?;
    for args in [
        vec!["config", "list"],
        vec!["config", "-l"],
        vec!["config", "--list"],
    ] {
        let output = ctx.command(&args, repo.clone(), true)?;
        assert_stdout_contains(&output, "user.name")?;
    }
    let names = ctx.command(&["config", "list", "--name-only"], repo.clone(), true)?;
    assert_stdout_contains(&names, "user.name")?;
    if String::from_utf8_lossy(&names.stdout).contains("List User") {
        bail!("config list --name-only leaked values");
    }
    let origin = ctx.command(&["config", "list", "--show-origin"], repo.clone(), true)?;
    assert_stdout_contains(&origin, "user.name")?;
    ctx.command(&["config", "--list", "--show-origin"], repo.clone(), true)?;
    ctx.command(&["config", "list", "--vault"], repo.clone(), true)?;
    let ssh = ctx.command(&["config", "list", "--ssh-keys"], repo.clone(), true)?;
    assert_not_contains(&ssh, "PRIVATE KEY")?;
    let gpg = ctx.command(&["config", "list", "--gpg-keys"], repo.clone(), true)?;
    assert_not_contains(&gpg, "PRIVATE KEY")?;
    let json_list = ctx.command(&["--json", "config", "list"], repo.clone(), true)?;
    assert_json_ok(&json_list, "config")?;
    let json_name_only = ctx.command(&["--json", "config", "list", "--name-only"], repo, true)?;
    assert_json_ok(&json_name_only, "config")?;
    assert_stdout_contains(&json_name_only, "user.name")?;
    Ok(())
}
