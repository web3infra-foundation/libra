use super::prelude::*;

pub(crate) fn scenario_init_template(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let template = ctx.run_dir.join("template");
    fs::create_dir_all(template.join("info")).context("create template info")?;
    fs::create_dir_all(template.join("hooks")).context("create template hooks")?;
    fs::create_dir_all(template.join("custom")).context("create template custom")?;
    fs::write(template.join("info/exclude"), "ignored-by-template\n")?;
    fs::write(template.join("hooks/pre-commit.sh"), "#!/bin/sh\nexit 0\n")?;
    fs::write(template.join("custom/sentinel.txt"), "sentinel\n")?;
    ctx.command(
        &["init", "--template", "template", "templated-repo"],
        ctx.run_dir.clone(),
        true,
    )?;
    let repo = ctx.run_dir.join("templated-repo");
    ensure_file(repo.join(".libra/info/exclude"))?;
    ensure_file(repo.join(".libra/hooks/pre-commit.sh"))?;
    ensure_file(repo.join(".libra/custom/sentinel.txt"))?;
    ctx.command(&["status"], repo.clone(), true)?;
    ctx.command(&["fsck", "--connectivity-only"], repo, true)?;
    let json = ctx.command(
        &["--json", "init", "--template", "template", "templated-json"],
        ctx.run_dir.clone(),
        true,
    )?;
    assert_json_ok(&json, "init")?;
    let missing = ctx.command(
        &[
            "init",
            "--template",
            "missing-template",
            "bad-template-repo",
        ],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&missing, "missing-template")?;
    Ok(())
}
