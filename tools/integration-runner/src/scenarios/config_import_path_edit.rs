use super::prelude::*;

pub(crate) fn scenario_config_import_path_edit(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let git_source = ctx.run_dir.join("git-config-source");
    fs::create_dir_all(&git_source).context("create git config source")?;
    ctx.gitfix(&["init"], git_source.clone(), true)?;
    ctx.gitfix(
        &["config", "user.name", "Imported Git User"],
        git_source.clone(),
        true,
    )?;
    ctx.gitfix(
        &["config", "user.email", "imported@example.invalid"],
        git_source.clone(),
        true,
    )?;
    ctx.command(&["init", "libra-import-target"], git_source.clone(), true)?;
    let target = git_source.join("libra-import-target");
    ctx.command(&["config", "import"], target.clone(), true)?;
    let name = ctx.command(&["config", "get", "user.name"], target.clone(), true)?;
    assert_stdout_contains(&name, "Imported Git User")?;
    let email = ctx.command(&["config", "get", "user.email"], target.clone(), true)?;
    assert_stdout_contains(&email, "imported@example.invalid")?;
    let path = ctx.command(&["config", "path"], target.clone(), true)?;
    let config_path = stdout_trim(&path);
    if config_path.is_empty() || !Path::new(&config_path).exists() {
        bail!("config path did not point at an existing file: {config_path}");
    }

    ctx.command(&["init", "libra-import-legacy"], git_source.clone(), true)?;
    let legacy = git_source.join("libra-import-legacy");
    ctx.command(&["config", "--import"], legacy.clone(), true)?;
    let legacy_name = ctx.command(&["config", "get", "user.name"], legacy.clone(), true)?;
    assert_stdout_contains(&legacy_name, "Imported Git User")?;
    let edit = ctx.command(&["config", "edit"], legacy.clone(), false)?;
    assert_lbr_or_text(&edit, "set/unset/list")?;
    let json_path = ctx.command(&["--json", "config", "path"], legacy.clone(), true)?;
    assert_json_ok(&json_path, "config")?;
    Ok(())
}
