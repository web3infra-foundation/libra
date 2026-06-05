use super::prelude::*;

pub(crate) fn scenario_init_from_git_repository(ctx: &mut ScenarioCtx<'_>) -> Result<()> {
    let git_source = ctx.run_dir.join("git-source");
    fs::create_dir_all(&git_source).context("create git source")?;
    ctx.gitfix(&["init"], git_source.clone(), true)?;
    ctx.gitfix(
        &["config", "user.name", "Git Fixture"],
        git_source.clone(),
        true,
    )?;
    ctx.gitfix(
        &["config", "user.email", "git-fixture@example.invalid"],
        git_source.clone(),
        true,
    )?;
    fs::write(git_source.join("README.md"), "from git\n").context("write git README")?;
    ctx.gitfix(&["add", "README.md"], git_source.clone(), true)?;
    ctx.gitfix(
        &["commit", "-m", "fixture: initial"],
        git_source.clone(),
        true,
    )?;
    ctx.command(
        &["init", "--from-git-repository", "git-source", "converted"],
        ctx.run_dir.clone(),
        true,
    )?;
    let converted = ctx.run_dir.join("converted");
    ctx.command(&["status"], converted.clone(), true)?;
    let log = ctx.command(&["log", "--oneline"], converted.clone(), true)?;
    assert_stdout_contains(&log, "fixture: initial")?;
    let readme =
        fs::read_to_string(converted.join("README.md")).context("read converted README")?;
    if !readme.contains("from git") {
        bail!("converted README did not match fixture: {readme}");
    }
    let json_status = ctx.command(&["--json", "status"], converted.clone(), true)?;
    assert_json_ok(&json_status, "status")?;
    ctx.command(&["fsck", "--connectivity-only"], converted, true)?;
    let missing = ctx.command(
        &[
            "init",
            "--from-git-repository",
            "missing-source",
            "converted-missing",
        ],
        ctx.run_dir.clone(),
        false,
    )?;
    assert_lbr_or_text(&missing, "Git repository")?;
    Ok(())
}
