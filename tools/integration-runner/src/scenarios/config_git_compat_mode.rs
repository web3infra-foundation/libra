use super::prelude::*;

pub(crate) fn scenario_config_git_compat_mode(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let repo = ctx.repo("config-repo");
    ctx.command(&["init", "config-repo"], ctx.run_dir.clone(), true)?;
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
    ctx.command(
        &["config", "user.compat", "value-from-positional"],
        repo.clone(),
        true,
    )?;
    let compat = ctx.command(&["config", "--get", "user.compat"], repo.clone(), true)?;
    assert_stdout_contains(&compat, "value-from-positional")?;
    ctx.command(
        &["config", "--add", "user.compat", "second-value"],
        repo.clone(),
        true,
    )?;
    let all = ctx.command(&["config", "--get-all", "user.compat"], repo.clone(), true)?;
    assert_stdout_contains(&all, "value-from-positional")?;
    assert_stdout_contains(&all, "second-value")?;
    let regexp = ctx.command(&["config", "--get-regexp", "^user\\."], repo.clone(), true)?;
    assert_stdout_contains(&regexp, "user.compat")?;
    ctx.command(&["config", "--list"], repo.clone(), true)?;
    ctx.command(&["config", "-l"], repo.clone(), true)?;
    ctx.command(
        &["config", "--unset-all", "user.compat"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "--unset-all", "remote.origin.fetch"],
        repo.clone(),
        true,
    )?;
    let fallback = ctx.command(
        &["config", "--get", "-d", "fallback", "missing.compat"],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&fallback, "fallback")?;
    let fallback_long = ctx.command(
        &[
            "config",
            "--get",
            "--default",
            "fallback-long",
            "missing.compat.long",
        ],
        repo.clone(),
        true,
    )?;
    assert_stdout_contains(&fallback_long, "fallback-long")?;
    ctx.command(
        &["config", "set", "custom.boolflag", "yes"],
        repo.clone(),
        true,
    )?;
    let bad_bool = ctx.command(
        &["config", "--bool", "--get", "custom.boolflag"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_bool, "--bool")?;
    ctx.command(
        &["config", "set", "temp.section.alpha", "one"],
        repo.clone(),
        true,
    )?;
    ctx.command(
        &["config", "set", "temp.section.beta", "two"],
        repo.clone(),
        true,
    )?;
    let bad_remove_section = ctx.command(
        &["config", "--remove-section", "temp.section"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_remove_section, "--remove-section")?;
    // Rejected compat flags with no other black-box coverage.
    let bad_url = ctx.command(
        &[
            "config",
            "--url",
            "https://example.invalid",
            "--get",
            "user.compat",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_url, "--url")?;
    let bad_no_show_names = ctx.command(
        &["config", "--no-show-names", "--get", "user.compat"],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_no_show_names, "--no-show-names")?;
    let bad_default = ctx.command(
        &[
            "config",
            "--default",
            "fallback",
            "user.bad-default",
            "value",
        ],
        repo.clone(),
        false,
    )?;
    assert_lbr_or_text(&bad_default, "default")?;
    let bad_top = ctx.command(&["config", "init", "value"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_top, "top-level")?;
    let bad_import_arg = ctx.command(&["config", "--import", "user.name"], repo.clone(), false)?;
    assert_lbr_or_text(&bad_import_arg, "import")?;
    assert_json_ok(
        &ctx.command(&["--json", "config", "list"], repo.clone(), true)?,
        "config",
    )?;
    Ok(())
}
